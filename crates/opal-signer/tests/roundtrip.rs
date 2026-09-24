//! End-to-end: a local relay, our signer, and rust-nostr's NIP-46 client
//! playing the part of an app.

use std::sync::Arc;
use std::time::Duration;

use nostr_connect::client::NostrConnect;
use nostr_sdk::prelude::*;
use opal_core::config::Policy;
use opal_core::keystore::SecretStore;
use opal_core::vault::Vault;
use opal_signer::{
    AllowAll, Approver, DenyAll, NostrConnectUri, Signer, SignerEvent, SignerSettings,
};

const TIMEOUT: Duration = Duration::from_secs(10);

struct Setup {
    _relay: MockRelay,
    relay_url: RelayUrl,
    vault: Arc<Vault>,
    account: PublicKey,
    signer: Signer,
}

async fn setup(approver: Arc<dyn Approver>, unlocked: bool, pending: Duration) -> Setup {
    let relay = MockRelay::run().await.unwrap();
    let relay_url = relay.url().await;
    let vault = Arc::new(Vault::with_log_n(SecretStore::memory(), 4));
    let account = vault.add_account(Keys::generate(), "pw").await.unwrap();
    if unlocked {
        vault.unlock("pw").await.unwrap();
    }
    let signer = Signer::new(
        vault.clone(),
        approver,
        SignerSettings {
            default_relays: vec![relay_url.clone()],
            pending_timeout: pending,
            log_activity: false,
        },
    );
    signer.start().await.unwrap();
    Setup {
        _relay: relay,
        relay_url,
        vault,
        account,
        signer,
    }
}

async fn bunker_client(s: &Setup) -> (NostrConnect, String) {
    let (_, uri) = s
        .signer
        .create_bunker(s.account, Some("test".into()), None, Policy::Basic, None)
        .await
        .unwrap();
    let parsed = nostr_connect::prelude::NostrConnectUri::parse(&uri).unwrap();
    (
        NostrConnect::new(parsed, Keys::generate(), TIMEOUT, None).unwrap(),
        uri,
    )
}

#[tokio::test]
async fn bunker_flow_signs_and_encrypts() {
    let s = setup(Arc::new(AllowAll), true, TIMEOUT).await;
    let mut events = s.signer.subscribe_events();
    let (app, _) = bunker_client(&s).await;

    assert_eq!(app.get_public_key_async().await.unwrap(), s.account);
    assert!(matches!(
        events.recv().await.unwrap(),
        SignerEvent::Connected { .. }
    ));

    let note = EventBuilder::new(Kind::TextNote, "hello from opal")
        .finalize_async(&app)
        .await
        .unwrap();
    note.verify().unwrap();
    assert_eq!(note.pubkey, s.account);
    assert_eq!(note.content, "hello from opal");

    let peer = Keys::generate();
    let ct = app
        .nip44_encrypt_async(&peer.public_key(), "secret msg")
        .await
        .unwrap();
    // The peer can read what we encrypted...
    assert_eq!(
        nip44::decrypt(peer.secret_key(), &s.account, &ct).unwrap(),
        "secret msg"
    );
    // ...and we can read it back through the signer.
    assert_eq!(
        app.nip44_decrypt_async(&peer.public_key(), &ct)
            .await
            .unwrap(),
        "secret msg"
    );
    let ct04 = app
        .nip04_encrypt_async(&peer.public_key(), "old style")
        .await
        .unwrap();
    assert_eq!(
        app.nip04_decrypt_async(&peer.public_key(), &ct04)
            .await
            .unwrap(),
        "old style"
    );

    let conns = s.signer.connections().await;
    assert_eq!(conns.len(), 1);
    assert!(conns[0].connected);
}

#[tokio::test]
async fn bunker_secret_is_single_use() {
    let s = setup(Arc::new(AllowAll), true, TIMEOUT).await;
    let (app, uri) = bunker_client(&s).await;
    app.get_public_key_async().await.unwrap();

    // A different client reusing the same URI must be refused.
    let parsed = nostr_connect::prelude::NostrConnectUri::parse(&uri).unwrap();
    let thief = NostrConnect::new(parsed, Keys::generate(), Duration::from_secs(3), None).unwrap();
    assert!(thief.get_public_key_async().await.is_err());
}

#[tokio::test]
async fn denied_requests_fail() {
    let s = setup(Arc::new(DenyAll), true, TIMEOUT).await;
    let (app, _) = bunker_client(&s).await;
    let err = EventBuilder::new(Kind::TextNote, "nope")
        .finalize_async(&app)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("rejected"), "{err}");
}

#[tokio::test]
async fn locked_vault_waits_then_fails() {
    let s = setup(Arc::new(AllowAll), false, Duration::from_millis(500)).await;
    let mut events = s.signer.subscribe_events();
    let (app, _) = bunker_client(&s).await;
    let err = app.get_public_key_async().await.unwrap_err();
    assert!(err.to_string().contains("locked"), "{err}");
    let mut saw_unlock_needed = false;
    while let Ok(e) = events.try_recv() {
        saw_unlock_needed |= matches!(e, SignerEvent::UnlockNeeded { .. });
    }
    assert!(saw_unlock_needed);
}

#[tokio::test]
async fn locked_vault_unlocked_in_time_succeeds() {
    let s = setup(Arc::new(AllowAll), false, TIMEOUT).await;
    let (app, _) = bunker_client(&s).await;
    let vault = s.vault.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        vault.unlock("pw").await.unwrap();
    });
    assert_eq!(app.get_public_key_async().await.unwrap(), s.account);
}

#[tokio::test]
async fn nostrconnect_flow() {
    let s = setup(Arc::new(AllowAll), true, TIMEOUT).await;
    let app_keys = Keys::generate();
    // rust-nostr's client only understands the legacy `metadata=` form, so
    // this also covers our parsing of it.
    let client_uri = nostr_connect::prelude::NostrConnectUri::client_with_secret(
        app_keys.public_key(),
        [s.relay_url.clone()],
        "Test App",
        "abc123",
    );
    let uri = client_uri.to_string();
    let app = NostrConnect::new(client_uri, app_keys, TIMEOUT, None).unwrap();

    // The app starts waiting for our response before we accept.
    let waiting = tokio::spawn(async move {
        let pk = app.get_public_key_async().await;
        (app, pk)
    });
    tokio::time::sleep(Duration::from_millis(500)).await;

    let ours = NostrConnectUri::parse(&uri).unwrap();
    let info = s
        .signer
        .accept_nostrconnect(&ours, s.account, Policy::Basic, &[])
        .await
        .unwrap();
    assert_eq!(info.name.as_deref(), Some("Test App"));

    let (app, pk) = waiting.await.unwrap();
    assert_eq!(pk.unwrap(), s.account);
    let note = EventBuilder::new(Kind::TextNote, "via nostrconnect")
        .finalize_async(&app)
        .await
        .unwrap();
    assert_eq!(note.pubkey, s.account);
}
