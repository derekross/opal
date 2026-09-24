//! How Opal's own modules sign: with the local vault, or through an external
//! NIP-46 bunker (e.g. Amber on your phone).

use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use nostr_connect::client::NostrConnect;
use nostr_connect::prelude::NostrConnectUri;
use nostr_sdk::prelude::*;
use opal_core::keystore::ItemKind;
use opal_core::vault::Vault;
use opal_signer::SignerStore;
use opal_signer::permissions::Source;
use opal_signer::protocol::Method;
use opal_signer::store::ActivityEntry;
use opal_status::StatusSigner;
use opal_status::engine::SignError;

/// Waiting for approval on a phone can take a while.
const BUNKER_TIMEOUT: Duration = Duration::from_secs(90);
const CLIENT_KEY_ID: &str = "external-bunker";

/// Signs with a key in the vault; unavailable while locked. Every signature
/// shows up in the activity log as "Opal Status".
pub struct LocalSigner {
    pub vault: Arc<Vault>,
    pub account: PublicKey,
    pub log: Option<SignerStore>,
}

impl StatusSigner for LocalSigner {
    fn sign(&self, unsigned: UnsignedEvent) -> BoxFuture<'_, Result<Event, SignError>> {
        Box::pin(async move {
            let keys = self
                .vault
                .keys(&self.account)
                .await
                .map_err(|e| SignError::Unavailable(e.to_string()))?;
            let kind = unsigned.kind.as_u16();
            let ev = keys
                .sign_event(unsigned)
                .map_err(|e| SignError::Failed(e.to_string()))?;
            if let Some(log) = &self.log {
                let _ = log.log_activity(&ActivityEntry {
                    id: 0,
                    at: Timestamp::now().as_secs(),
                    app_id: "opal-status".into(),
                    app_name: "Opal Status".into(),
                    account: self.account.to_hex(),
                    method: Method::SignEvent,
                    kind: Some(kind),
                    kind_label: None,
                    allowed: true,
                    source: Source::Automatic,
                    reason: None,
                });
            }
            Ok(ev)
        })
    }
}

/// Signs through a remote bunker.
pub struct BunkerSigner {
    client: NostrConnect,
}

impl BunkerSigner {
    /// Connect to `uri` with our persistent client key (so the bunker keeps
    /// recognising us after restarts). Returns the user's public key.
    pub async fn connect(vault: &Vault, uri: &str) -> anyhow::Result<(Self, PublicKey)> {
        let parsed = NostrConnectUri::parse(uri.trim())
            .map_err(|e| anyhow::anyhow!("not a bunker:// URI: {e}"))?;
        if !parsed.is_bunker() {
            anyhow::bail!("paste a bunker:// URI from your signer app");
        }
        let keys = client_keys(vault).await?;
        let client = NostrConnect::new(parsed, keys, BUNKER_TIMEOUT, None)?;
        let pk = client
            .get_public_key_async()
            .await
            .map_err(|e| anyhow::anyhow!("the bunker didn't answer: {e}"))?;
        Ok((Self { client }, pk))
    }
}

async fn client_keys(vault: &Vault) -> anyhow::Result<Keys> {
    let store = vault.store();
    if let Some(hex) = store.get(ItemKind::ClientKey, CLIENT_KEY_ID).await?
        && let Ok(keys) = Keys::parse(&hex)
    {
        return Ok(keys);
    }
    let keys = Keys::generate();
    store
        .put(
            ItemKind::ClientKey,
            CLIENT_KEY_ID,
            "Opal client key for your external signer",
            &keys.secret_key().to_secret_hex(),
        )
        .await?;
    Ok(keys)
}

impl StatusSigner for BunkerSigner {
    fn sign(&self, unsigned: UnsignedEvent) -> BoxFuture<'_, Result<Event, SignError>> {
        Box::pin(async move {
            self.client.sign_event_async(unsigned).await.map_err(|e| {
                let msg = e.to_string();
                if msg.contains("reject") || msg.contains("denied") {
                    SignError::Failed(msg)
                } else {
                    // Offline or timed out: keep it for later.
                    SignError::Unavailable(msg)
                }
            })
        })
    }
}
