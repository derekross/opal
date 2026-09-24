//! Connections: one per app that talks to the signer.

use std::collections::HashMap;

use nostr_sdk::prelude::{Keys, PublicKey, RelayUrl, Timestamp};
use opal_core::config::Policy;
use serde::Serialize;
use tokio::sync::RwLock;

use crate::perms::PermSpec;

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
    /// Single-use `bunker://` secret, cleared after the first `connect`.
    pub secret: Option<String>,
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

/// What the UI and IPC see; never contains key material.
#[derive(Debug, Clone, Serialize)]
pub struct ConnectionInfo {
    pub id: String,
    pub account: String,
    pub signer_pubkey: String,
    pub client: Option<String>,
    pub connected: bool,
    pub name: Option<String>,
    pub url: Option<String>,
    pub image: Option<String>,
    pub relays: Vec<String>,
    pub policy: Policy,
    pub requested_perms: Vec<PermSpec>,
    pub created_at: u64,
    pub last_used: Option<u64>,
}

impl Connection {
    pub fn display_name(&self) -> String {
        self.name
            .clone()
            .or_else(|| self.url.clone())
            .unwrap_or_else(|| format!("App {}", &self.id[..8.min(self.id.len())]))
    }

    pub fn info(&self) -> ConnectionInfo {
        ConnectionInfo {
            id: self.id.clone(),
            account: self.account.to_hex(),
            signer_pubkey: self.transport.public_key().to_hex(),
            client: self.client.map(|c| c.to_hex()),
            connected: self.client.is_some(),
            name: self.name.clone(),
            url: self.url.clone(),
            image: self.image.clone(),
            relays: self.relays.iter().map(|r| r.to_string()).collect(),
            policy: self.policy,
            requested_perms: self.requested_perms.clone(),
            created_at: self.created_at.as_secs(),
            last_used: self.last_used.map(|t| t.as_secs()),
        }
    }
}

/// Connections indexed by their transport public key.
#[derive(Default)]
pub struct ConnStore {
    by_transport: RwLock<HashMap<PublicKey, Connection>>,
}

impl ConnStore {
    pub async fn insert(&self, conn: Connection) {
        self.by_transport
            .write()
            .await
            .insert(conn.transport.public_key(), conn);
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

    /// Apply `f` to the connection and return the updated copy.
    pub async fn update<F>(&self, transport: &PublicKey, f: F) -> Option<Connection>
    where
        F: FnOnce(&mut Connection),
    {
        let mut map = self.by_transport.write().await;
        let conn = map.get_mut(transport)?;
        f(conn);
        Some(conn.clone())
    }

    pub async fn remove(&self, id: &str) -> Option<Connection> {
        let mut map = self.by_transport.write().await;
        let key = *map.iter().find(|(_, c)| c.id == id)?.0;
        map.remove(&key)
    }

    pub async fn all(&self) -> Vec<Connection> {
        let mut v: Vec<_> = self.by_transport.read().await.values().cloned().collect();
        v.sort_by_key(|c| std::cmp::Reverse(c.created_at));
        v
    }

    pub async fn transport_keys(&self) -> Vec<PublicKey> {
        self.by_transport.read().await.keys().copied().collect()
    }
}
