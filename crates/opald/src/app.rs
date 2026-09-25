//! Daemon state shared by the IPC server and background tasks.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use nostr_sdk::prelude::{PublicKey, RelayUrl, ToBech32};
use opal_core::accounts::Accounts;
use opal_core::config::Config;
use opal_core::db::Db;
use opal_core::ipc::IpcEvent;
use opal_core::keystore::SecretStore;
use opal_core::vault::Vault;
use opal_notify::{NotifyEngine, NotifyStore};
use opal_status::{StatusEngine, StatusStore};

use crate::signers::BunkerSigner;
use opal_signer::{
    NostrConnectUri, PolicyApprover, PromptHub, Signer, SignerSettings, SignerStore,
};
use serde_json::{Value, json};
use tokio::sync::{Mutex, RwLock, broadcast};

pub struct App {
    pub config: RwLock<Config>,
    /// Where `config` was loaded from; settings are saved back there.
    pub config_path: std::path::PathBuf,
    pub vault: Arc<Vault>,
    pub accounts: Accounts,
    pub signer: Signer,
    pub prompts: Arc<PromptHub>,
    pub events: broadcast::Sender<IpcEvent>,
    /// Last time the vault was used; drives auto-lock.
    last_activity: Mutex<Instant>,
    online: AtomicBool,
    /// The NIP-46 signer has connected to its relays at least once.
    pub signer_started: AtomicBool,
    /// `nostrconnect://` URIs handed to us (xdg handler, CLI) awaiting the UI.
    pub offers: Mutex<HashMap<String, NostrConnectUri>>,
    pub notify_store: NotifyStore,
    pub status_store: StatusStore,
    /// The running status module and what it was started with.
    pub status: Mutex<Option<(StatusEngine, String)>>,
    /// Connected external bunker (identity mode "external").
    pub bunker: Mutex<Option<Arc<BunkerSigner>>>,
    /// Why the status module isn't running, for the UI.
    pub status_blocked: Mutex<Option<String>>,
    /// The running notifications module and what it was started with.
    pub notify: Mutex<Option<(NotifyEngine, String)>>,
}

pub struct Options {
    pub config: Config,
    pub config_path: std::path::PathBuf,
    pub db: Db,
    pub store: SecretStore,
}

impl App {
    pub async fn new(opts: Options) -> Result<Arc<Self>> {
        let Options {
            config,
            config_path,
            db,
            store,
        } = opts;
        let vault = Arc::new(Vault::new(store));
        let accounts = Accounts::new(db.clone()).context("accounts table")?;
        // Keep account details in step with the keyring (e.g. after a restore).
        let now = nostr_sdk::prelude::Timestamp::now().as_secs();
        for pk in vault.accounts().await.context("reading keyring")? {
            if accounts.get(&pk)?.is_none() {
                accounts.add(&pk, None, now)?;
            }
        }

        let signer_store = SignerStore::new(db.clone()).context("signer tables")?;
        let prompts = Arc::new(PromptHub::default());
        let approver = Arc::new(PolicyApprover::new(signer_store.clone(), prompts.clone()));
        let signer = Signer::with_store(
            vault.clone(),
            approver,
            signer_settings(&config),
            signer_store,
        )
        .await
        .context("loading apps")?;

        let notify_store = NotifyStore::new(db.clone()).context("notification tables")?;
        let status_store = StatusStore::new(db.clone()).context("status tables")?;
        let (events, _) = broadcast::channel(256);
        Ok(Arc::new(Self {
            config: RwLock::new(config),
            config_path,
            vault,
            accounts,
            signer,
            prompts,
            events,
            last_activity: Mutex::new(Instant::now()),
            online: AtomicBool::new(true),
            signer_started: AtomicBool::new(false),
            offers: Mutex::new(HashMap::new()),
            notify_store,
            notify: Mutex::new(None),
            status_store,
            status: Mutex::new(None),
            bunker: Mutex::new(None),
            status_blocked: Mutex::new(None),
        }))
    }

    pub fn emit(&self, event: &str, data: Value) {
        let _ = self.events.send(IpcEvent {
            event: event.to_string(),
            data,
        });
    }

    pub async fn touch(&self) {
        *self.last_activity.lock().await = Instant::now();
    }

    pub async fn idle_for(&self) -> Duration {
        self.last_activity.lock().await.elapsed()
    }

    pub fn is_online(&self) -> bool {
        self.online.load(Ordering::Relaxed)
    }

    pub async fn set_online(&self, online: bool) {
        self.online.store(online, Ordering::Relaxed);
        // The signer only answers apps while its module is on.
        let signer_on = self.config.read().await.modules.signer;
        if self.signer_started.load(Ordering::Relaxed) {
            self.signer.set_online(online && signer_on).await;
        }
        self.emit_state().await;
    }

    pub async fn lock(&self, reason: &str) {
        if self.vault.is_unlocked() {
            self.vault.lock().await;
            tracing::info!("locked ({reason})");
            self.emit_state().await;
        }
    }

    /// The account new connections use unless told otherwise.
    pub fn current_account(&self) -> Result<Option<PublicKey>> {
        Ok(self.accounts.current()?)
    }

    pub async fn status(&self) -> Value {
        let cfg = self.config.read().await;
        let current = self.accounts.current().ok().flatten();
        // Only accounts whose key is actually in the keyring.
        let in_keyring: Vec<String> = self
            .vault
            .accounts()
            .await
            .unwrap_or_default()
            .iter()
            .map(|pk| pk.to_hex())
            .collect();
        let accounts: Vec<Value> = self
            .accounts
            .list()
            .unwrap_or_default()
            .into_iter()
            .filter(|a| in_keyring.contains(&a.pubkey))
            .map(|a| {
                let npub = PublicKey::from_hex(&a.pubkey)
                    .ok()
                    .and_then(|pk| pk.to_bech32().ok());
                let is_current = current.is_some_and(|c| c.to_hex() == a.pubkey);
                let label = a.label();
                let mut v = serde_json::to_value(&a).unwrap_or_default();
                v["npub"] = json!(npub);
                v["label"] = json!(label);
                v["current"] = json!(is_current);
                v
            })
            .collect();
        // In read-only mode, who is being watched (profile from the notifications cache).
        let watched = cfg
            .identity
            .npub
            .as_deref()
            // A saved read-only profile (in use or not); in external mode the
            // npub belongs to the external signer instead.
            .filter(|_| cfg.identity.mode != opal_core::config::IdentityMode::External)
            .and_then(|n| PublicKey::parse(n).ok())
            .map(|pk| {
                let profile = self.notify_store.profile(&pk.to_hex()).ok().flatten().map(|(p, _)| p);
                json!({
                    "active": cfg.identity.mode == opal_core::config::IdentityMode::ReadOnly,
                    "npub": pk.to_bech32().ok(),
                    "nip05": cfg.identity.nip05,
                    "name": profile.as_ref().and_then(|p| p.display_name.clone().or_else(|| p.name.clone())),
                    "picture": profile.as_ref().and_then(|p| p.picture.clone()),
                })
            });
        json!({
            "watched": watched,
            "locked": !self.vault.is_unlocked(),
            "online": self.is_online(),
            "has_accounts": !accounts.is_empty(),
            "accounts": accounts,
            "current_account": current.map(|c| c.to_hex()),
            "identity": cfg.identity,
            "modules": cfg.modules,
            "pending_prompts": self.prompts.pending().len(),
            "unread_notifications": self.unread_notifications().await,
            "status_running": self.status.lock().await.is_some(),
            "status_blocked": *self.status_blocked.lock().await,
            "auto_lock_minutes": cfg.signer.auto_lock_minutes,
        })
    }

    /// Unread count for the account the notifications module is watching.
    pub async fn unread_notifications(&self) -> u64 {
        let guard = self.notify.lock().await;
        match guard.as_ref() {
            Some((engine, _)) => self
                .notify_store
                .unread_count(&engine.account().to_hex())
                .unwrap_or(0),
            None => 0,
        }
    }

    pub async fn emit_state(&self) {
        let s = self.status().await;
        self.emit("state", s);
    }
}

pub fn signer_settings(cfg: &Config) -> SignerSettings {
    SignerSettings {
        default_relays: parse_relays(&cfg.signer.relays),
        pending_timeout: Duration::from_secs(cfg.signer.pending_timeout_secs.max(10)),
        log_activity: !cfg.signer.privacy_mode,
    }
}

pub fn parse_relays(list: &[String]) -> Vec<RelayUrl> {
    list.iter()
        .filter_map(|r| match RelayUrl::parse(r) {
            Ok(u) => Some(u),
            Err(e) => {
                tracing::warn!("ignoring relay {r}: {e}");
                None
            }
        })
        .collect()
}
