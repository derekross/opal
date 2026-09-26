//! Connections: one per app that talks to the signer.

use std::collections::HashMap;
use std::sync::Arc;

use nostr_sdk::prelude::{Keys, PublicKey, RelayUrl, Timestamp};
use opal_core::config::Policy;
use opal_core::keystore::ItemKind;
use opal_core::vault::Vault;
use serde::Serialize;
use tokio::sync::RwLock;

use crate::perms::PermSpec;
use crate::store::{AppRecord, SignerStore, hash_secret};

#[derive(Debug, Clone)]
pub struct Connection {
    pub id: String,
    /// The user account this app signs as.
    pub account: PublicKey,
    /// Our key for this connection only (the "remote-signer" key in NIP-46),
    /// so apps don't learn the user's npub until `get_public_key`.
    pub transport: Keys,
    /// The client's key once connected.
    pub client: Option<PublicKey>,
    /// SHA-256 of the single-use `bunker://` secret; cleared after `connect`.
    pub secret_hash: Option<String>,
    pub name: Option<String>,
    pub url: Option<String>,
    pub image: Option<String>,
    pub relays: Vec<RelayUrl>,
    pub policy: Policy,
    pub requested_perms: Vec<PermSpec>,
    pub created_at: Timestamp,
    pub last_used: Option<Timestamp>,
    /// Remove the connection if nobody connected by then.
    pub expires_unused_at: Option<Timestamp>,
}

/// How an app reaches the signer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AppKind {
    /// Over relays, NIP-46.
    Nip46,
    /// A program on this computer, over the control socket.
    Local,
}

/// What the UI and IPC see; never contains key material.
#[derive(Debug, Clone, Serialize)]
pub struct ConnectionInfo {
    pub id: String,
    pub kind: AppKind,
    pub account: String,
    /// The NIP-46 transport key; local apps have none.
    pub signer_pubkey: Option<String>,
    pub client: Option<String>,
    pub connected: bool,
    pub name: Option<String>,
    pub display_name: String,
    pub url: Option<String>,
    pub image: Option<String>,
    pub relays: Vec<String>,
    pub policy: Policy,
    pub requested_perms: Vec<PermSpec>,
    pub created_at: u64,
    pub last_used: Option<u64>,
    pub expires_unused_at: Option<u64>,
    /// Local apps: the program it was paired from, the kinds it declared,
    /// and whether it may encrypt to the user's own key.
    pub exe: Option<String>,
    pub kinds: Vec<u16>,
    pub nip44: bool,
}

impl Connection {
    pub fn display_name(&self) -> String {
        self.name
            .clone()
            .or_else(|| self.url.clone())
            .unwrap_or_else(|| format!("App {}", &self.id[..8.min(self.id.len())]))
    }

    pub fn secret_matches(&self, given: &str) -> bool {
        !given.is_empty() && self.secret_hash.as_deref() == Some(hash_secret(given).as_str())
    }

    pub fn info(&self) -> ConnectionInfo {
        ConnectionInfo {
            id: self.id.clone(),
            kind: AppKind::Nip46,
            account: self.account.to_hex(),
            signer_pubkey: Some(self.transport.public_key().to_hex()),
            client: self.client.map(|c| c.to_hex()),
            connected: self.client.is_some(),
            name: self.name.clone(),
            display_name: self.display_name(),
            url: self.url.clone(),
            image: self.image.clone(),
            relays: self.relays.iter().map(|r| r.to_string()).collect(),
            policy: self.policy,
            requested_perms: self.requested_perms.clone(),
            created_at: self.created_at.as_secs(),
            last_used: self.last_used.map(|t| t.as_secs()),
            expires_unused_at: self.expires_unused_at.map(|t| t.as_secs()),
            exe: None,
            kinds: Vec::new(),
            nip44: false,
        }
    }

    fn record(&self) -> AppRecord {
        AppRecord {
            id: self.id.clone(),
            account: self.account,
            transport_pubkey: self.transport.public_key(),
            client: self.client,
            secret_hash: self.secret_hash.clone(),
            name: self.name.clone(),
            url: self.url.clone(),
            image: self.image.clone(),
            relays: self.relays.clone(),
            policy: self.policy,
            requested_perms: self.requested_perms.clone(),
            created_at: self.created_at,
            last_used: self.last_used,
            expires_unused_at: self.expires_unused_at,
        }
    }
}

struct Persist {
    store: SignerStore,
    vault: Arc<Vault>,
}

/// Connections indexed by their transport public key, optionally persisted
/// (details in SQLite, transport keys in the keyring).
#[derive(Default)]
pub struct ConnStore {
    by_transport: RwLock<HashMap<PublicKey, Connection>>,
    persist: Option<Persist>,
}

impl ConnStore {
    pub fn memory() -> Self {
        Self::default()
    }

    /// Load saved connections. Apps whose transport key is missing from the
    /// keyring are dropped, since nobody can talk to them any more.
    pub async fn load(store: SignerStore, vault: Arc<Vault>) -> opal_core::Result<Self> {
        let mut map = HashMap::new();
        for rec in store.load_apps()? {
            let key = vault.store().get(ItemKind::ConnKey, &rec.id).await?;
            let Some(transport) = key.and_then(|k| Keys::parse(&k).ok()) else {
                tracing::warn!(app = %rec.id, "transport key missing; removing app");
                store.delete_app(&rec.id)?;
                continue;
            };
            if transport.public_key() != rec.transport_pubkey {
                tracing::warn!(app = %rec.id, "transport key mismatch; removing app");
                store.delete_app(&rec.id)?;
                continue;
            }
            let conn = Connection {
                id: rec.id,
                account: rec.account,
                transport,
                client: rec.client,
                secret_hash: rec.secret_hash,
                name: rec.name,
                url: rec.url,
                image: rec.image,
                relays: rec.relays,
                policy: rec.policy,
                requested_perms: rec.requested_perms,
                created_at: rec.created_at,
                last_used: rec.last_used,
                expires_unused_at: rec.expires_unused_at,
            };
            map.insert(conn.transport.public_key(), conn);
        }
        Ok(Self {
            by_transport: RwLock::new(map),
            persist: Some(Persist { store, vault }),
        })
    }

    pub async fn insert(&self, conn: Connection) -> opal_core::Result<()> {
        if let Some(p) = &self.persist {
            let label = format!("Opal NIP-46 app key ({})", conn.display_name());
            p.vault
                .store()
                .put(
                    ItemKind::ConnKey,
                    &conn.id,
                    &label,
                    &conn.transport.secret_key().to_secret_hex(),
                )
                .await?;
            p.store.save_app(&conn.record())?;
        }
        self.by_transport
            .write()
            .await
            .insert(conn.transport.public_key(), conn);
        Ok(())
    }

    pub async fn get_by_transport(&self, pk: &PublicKey) -> Option<Connection> {
        self.by_transport.read().await.get(pk).cloned()
    }

    pub async fn get(&self, id: &str) -> Option<Connection> {
        self.by_transport
            .read()
            .await
            .values()
            .find(|c| c.id == id)
            .cloned()
    }

    /// Apply `f` to the connection, save it, and return the updated copy.
    pub async fn update<F>(&self, id: &str, f: F) -> Option<Connection>
    where
        F: FnOnce(&mut Connection),
    {
        let updated = {
            let mut map = self.by_transport.write().await;
            let conn = map.values_mut().find(|c| c.id == id)?;
            f(conn);
            conn.clone()
        };
        if let Some(p) = &self.persist
            && let Err(e) = p.store.save_app(&updated.record())
        {
            tracing::warn!("saving app {id} failed: {e}");
        }
        Some(updated)
    }

    pub async fn remove(&self, id: &str) -> Option<Connection> {
        let removed = {
            let mut map = self.by_transport.write().await;
            let key = *map.iter().find(|(_, c)| c.id == id)?.0;
            map.remove(&key)
        };
        if let Some(p) = &self.persist {
            if let Err(e) = p.store.delete_app(id) {
                tracing::warn!("deleting app {id} failed: {e}");
            }
            if let Err(e) = p.vault.store().delete(ItemKind::ConnKey, id).await {
                tracing::warn!("deleting key of app {id} failed: {e}");
            }
        }
        removed
    }

    pub async fn all(&self) -> Vec<Connection> {
        let mut v: Vec<_> = self.by_transport.read().await.values().cloned().collect();
        v.sort_by_key(|c| std::cmp::Reverse(c.created_at));
        v
    }

    pub async fn transport_keys(&self) -> Vec<PublicKey> {
        self.by_transport.read().await.keys().copied().collect()
    }

    pub async fn relays(&self) -> Vec<RelayUrl> {
        let mut out: Vec<RelayUrl> = Vec::new();
        for c in self.by_transport.read().await.values() {
            for r in &c.relays {
                if !out.contains(r) {
                    out.push(r.clone());
                }
            }
        }
        out
    }
}
