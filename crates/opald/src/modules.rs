//! Starting and stopping optional modules to match the settings.

use std::sync::Arc;

use nostr_sdk::prelude::PublicKey;
use opal_core::config::IdentityMode;
use opal_notify::{NotifyEngine, NotifyEvent, NotifyParams};
use opal_status::{StatusEngine, StatusEvent, StatusParams, StatusSigner};
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;

use crate::app::App;
use crate::notify;
use crate::signers::{BunkerSigner, LocalSigner};

/// Whose notifications to show, and whether the key is local.
async fn notify_identity(app: &App) -> Option<(PublicKey, bool)> {
    let cfg = app.config.read().await;
    match cfg.identity.mode {
        IdentityMode::Local => {
            let pk = app.current_account().ok().flatten()?;
            let local = app.vault.accounts().await.ok()?.contains(&pk);
            Some((pk, local))
        }
        IdentityMode::ReadOnly | IdentityMode::External => cfg
            .identity
            .npub
            .as_deref()
            .and_then(|n| PublicKey::parse(n).ok())
            .map(|pk| (pk, false)),
    }
}

/// Make every optional module match the config.
pub async fn reconcile(app: &Arc<App>) {
    reconcile_notify(app).await;
    reconcile_status(app).await;
}

/// Start, stop, or restart the notifications module when the account or
/// its settings changed.
async fn reconcile_notify(app: &Arc<App>) {
    let (enabled, ncfg) = {
        let cfg = app.config.read().await;
        (cfg.modules.notifications, cfg.notifications.clone())
    };
    let identity = if enabled {
        notify_identity(app).await
    } else {
        None
    };
    let wanted = identity.map(|(pk, local)| {
        let key = format!(
            "{}|{}|{}",
            pk.to_hex(),
            local,
            serde_json::to_string(&ncfg).unwrap_or_default()
        );
        (pk, local, key)
    });

    let mut slot = app.notify.lock().await;
    let current_key = slot.as_ref().map(|(_, k)| k.clone());
    if current_key.as_deref() == wanted.as_ref().map(|w| w.2.as_str()) {
        return;
    }
    if let Some((engine, _)) = slot.take() {
        engine.stop().await;
        tracing::info!("notifications stopped");
    }
    if let Some((pk, local, key)) = wanted {
        let engine = NotifyEngine::start(NotifyParams {
            account: pk,
            config: ncfg,
            store: app.notify_store.clone(),
            vault: local.then(|| app.vault.clone()),
        });
        tokio::spawn(forward(app.clone(), engine.subscribe()));
        tracing::info!("notifications started");
        *slot = Some((engine, key));
    }
    drop(slot);
    app.emit_state().await;
}

async fn forward(app: Arc<App>, mut rx: tokio::sync::broadcast::Receiver<NotifyEvent>) {
    loop {
        let ev = match rx.recv().await {
            Ok(e) => e,
            Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => break,
        };
        if let NotifyEvent::New {
            notification,
            fresh: true,
        } = &ev
        {
            let app = app.clone();
            let n = notification.clone();
            tokio::spawn(async move { notify::nostr_popup(&app, n).await });
        }
        app.emit("notify", serde_json::to_value(&ev).unwrap_or_default());
        if matches!(ev, NotifyEvent::Updated) {
            // Profiles arrived: the header may show the watched person now.
            app.emit_state().await;
        }
        if matches!(ev, NotifyEvent::New { .. }) {
            app.emit("unread", json!({"count": app.unread_notifications().await}));
        }
    }
}

/// What the status module should sign with, or why it can't run.
async fn status_signer(
    app: &Arc<App>,
) -> Result<(PublicKey, Arc<dyn StatusSigner>, bool, String), String> {
    let mode = app.config.read().await.identity.mode;
    match mode {
        IdentityMode::Local => {
            let pk = app
                .current_account()
                .ok()
                .flatten()
                .ok_or("add an account to publish statuses")?;
            if !app.vault.accounts().await.unwrap_or_default().contains(&pk) {
                return Err("add an account to publish statuses".into());
            }
            let signer = LocalSigner {
                vault: app.vault.clone(),
                account: pk,
                log: app.signer.store().cloned(),
            };
            Ok((pk, Arc::new(signer), true, format!("local:{}", pk.to_hex())))
        }
        IdentityMode::External => {
            let npub = app.config.read().await.identity.npub.clone();
            let pk = npub
                .as_deref()
                .and_then(|n| PublicKey::parse(n).ok())
                .ok_or("connect your external signer first")?;
            let bunker = app.bunker.lock().await.clone();
            match bunker {
                Some(b) => Ok((
                    pk,
                    b as Arc<dyn StatusSigner>,
                    false,
                    format!("external:{}", pk.to_hex()),
                )),
                None => Err("connecting to your external signer…".into()),
            }
        }
        IdentityMode::ReadOnly => Err(
            "watching someone is read-only; statuses need your key or an external signer".into(),
        ),
    }
}

/// Start, stop, or restart the status module.
async fn reconcile_status(app: &Arc<App>) {
    let (enabled, scfg, bootstrap) = {
        let cfg = app.config.read().await;
        (
            cfg.modules.status,
            cfg.status.clone(),
            cfg.notifications.bootstrap_relays.clone(),
        )
    };
    let wanted = if enabled {
        match status_signer(app).await {
            Ok(w) => {
                *app.status_blocked.lock().await = None;
                Some(w)
            }
            Err(why) => {
                *app.status_blocked.lock().await = Some(why);
                None
            }
        }
    } else {
        *app.status_blocked.lock().await = None;
        None
    };
    let key = wanted
        .as_ref()
        .map(|(_, _, _, id)| format!("{id}|{}", serde_json::to_string(&scfg).unwrap_or_default()));

    let mut slot = app.status.lock().await;
    if slot.as_ref().map(|(_, k)| k) == key.as_ref() {
        return;
    }
    if let Some((engine, _)) = slot.take() {
        engine.stop().await;
        tracing::info!("status stopped");
    }
    if let (Some((pk, signer, local, _)), Some(key)) = (wanted, key) {
        let engine = StatusEngine::start(StatusParams {
            account: pk,
            config: scfg,
            store: app.status_store.clone(),
            signer,
            bootstrap_relays: bootstrap,
            signer_ready: local.then(|| app.vault.subscribe()),
        });
        tokio::spawn(forward_status(app.clone(), engine.subscribe()));
        tracing::info!("status started");
        *slot = Some((engine, key));
    }
    drop(slot);
    app.emit_state().await;
}

async fn forward_status(app: Arc<App>, mut rx: tokio::sync::broadcast::Receiver<StatusEvent>) {
    loop {
        match rx.recv().await {
            Ok(ev) => app.emit("status", serde_json::to_value(&ev).unwrap_or_default()),
            Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => break,
        }
    }
}

/// In external mode, connect to the bunker in the background at startup.
pub fn connect_bunker_in_background(app: &Arc<App>) {
    let app = app.clone();
    tokio::spawn(async move {
        let uri = {
            let cfg = app.config.read().await;
            if cfg.identity.mode != IdentityMode::External {
                return;
            }
            cfg.identity.bunker_uri.clone()
        };
        let Some(uri) = uri else { return };
        match BunkerSigner::connect(&app.vault, &uri).await {
            Ok((b, _)) => {
                *app.bunker.lock().await = Some(Arc::new(b));
                reconcile(&app).await;
            }
            Err(e) => {
                tracing::warn!("external signer: {e}");
                *app.status_blocked.lock().await = Some(format!("external signer: {e}"));
                app.emit_state().await;
            }
        }
    });
}
