//! How the apps' own features sign: with a key in memory, a key in Opal's
//! vault, or through an external NIP-46 bunker (e.g. Amber on a phone).

use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use nostr_connect::client::NostrConnect;
use nostr_connect::prelude::NostrConnectUri;
use nostr_sdk::prelude::*;
use opal_core::keystore::{ItemKind, SecretStore};
use opal_core::vault::Vault;

/// How long a feature waits for a signature before treating the signer as
/// unavailable for now.
pub const SIGN_TIMEOUT: Duration = Duration::from_secs(15);
/// Waiting for approval on a phone can take a while.
const BUNKER_TIMEOUT: Duration = Duration::from_secs(90);
const CLIENT_KEY_ID: &str = "external-bunker";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignError {
    /// The key isn't available right now (vault locked, bunker offline).
    Unavailable(String),
    Failed(String),
}

impl std::fmt::Display for SignError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(m) | Self::Failed(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for SignError {}

/// Signs events as the user.
pub trait EventSigner: Send + Sync {
    fn sign(&self, unsigned: UnsignedEvent) -> BoxFuture<'_, Result<Event, SignError>>;
}

/// Sign, but never wait longer than `timeout`: an external signer may be
/// slow or away, and callers usually drive other work too.
pub async fn sign_within(
    signer: &dyn EventSigner,
    unsigned: UnsignedEvent,
    timeout: Duration,
) -> Result<Event, SignError> {
    match tokio::time::timeout(timeout, signer.sign(unsigned)).await {
        Ok(r) => r,
        Err(_) => Err(SignError::Unavailable(
            "the signer didn't answer in time".into(),
        )),
    }
}

/// Signs with keys held in memory.
pub struct KeysSigner(pub Keys);

impl EventSigner for KeysSigner {
    fn sign(&self, unsigned: UnsignedEvent) -> BoxFuture<'_, Result<Event, SignError>> {
        Box::pin(async move {
            self.0
                .sign_event(unsigned)
                .map_err(|e| SignError::Failed(e.to_string()))
        })
    }
}

/// Called after each signature with the account and the event kind (for an
/// activity log).
pub type OnSign = Arc<dyn Fn(&PublicKey, u16) + Send + Sync>;

/// Signs with an account in Opal's vault; unavailable while locked.
pub struct LocalSigner {
    pub vault: Arc<Vault>,
    pub account: PublicKey,
    pub on_sign: Option<OnSign>,
}

impl EventSigner for LocalSigner {
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
            if let Some(cb) = &self.on_sign {
                cb(&self.account, kind);
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
    /// Connect to `uri` with our persistent client key from `store` (so the
    /// bunker keeps recognising us after restarts). Returns the user's
    /// public key.
    pub async fn connect(store: &SecretStore, uri: &str) -> anyhow::Result<(Self, PublicKey)> {
        let parsed = NostrConnectUri::parse(uri.trim())
            .map_err(|e| anyhow::anyhow!("not a bunker:// URI: {e}"))?;
        if !parsed.is_bunker() {
            anyhow::bail!("paste a bunker:// URI from your signer app");
        }
        let keys = client_keys(store).await?;
        let client = NostrConnect::new(parsed, keys, BUNKER_TIMEOUT, None)?;
        let pk = client
            .get_public_key_async()
            .await
            .map_err(|e| anyhow::anyhow!("the bunker didn't answer: {e}"))?;
        Ok((Self { client }, pk))
    }
}

async fn client_keys(store: &SecretStore) -> anyhow::Result<Keys> {
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

impl EventSigner for BunkerSigner {
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

#[cfg(test)]
mod tests {
    use super::*;

    struct Never;
    impl EventSigner for Never {
        fn sign(&self, _: UnsignedEvent) -> BoxFuture<'_, Result<Event, SignError>> {
            Box::pin(futures::future::pending())
        }
    }

    #[tokio::test]
    async fn keys_signer_signs_and_timeouts_are_unavailable() {
        let keys = Keys::generate();
        let unsigned = EventBuilder::new(Kind::TextNote, "hi").finalize_unsigned(keys.public_key());
        let ev = sign_within(&KeysSigner(keys.clone()), unsigned.clone(), SIGN_TIMEOUT)
            .await
            .unwrap();
        assert!(ev.verify().is_ok());
        let err = sign_within(&Never, unsigned, Duration::from_millis(20))
            .await
            .unwrap_err();
        assert!(matches!(err, SignError::Unavailable(_)));
    }
}
