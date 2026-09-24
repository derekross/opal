//! Starting and stopping optional modules to match the settings.

use std::sync::Arc;

use nostr_sdk::prelude::PublicKey;
use opal_core::config::IdentityMode;
use opal_notify::{NotifyEngine, NotifyEvent, NotifyParams};
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;

use crate::app::App;
use crate::notify;

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

/// Make the notifications module match the config: start, stop, or restart
/// it when the account or its settings changed.
pub async fn reconcile(app: &Arc<App>) {
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
        if matches!(ev, NotifyEvent::New { .. }) {
            app.emit("unread", json!({"count": app.unread_notifications().await}));
        }
    }
}
