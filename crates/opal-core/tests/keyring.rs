//! Talks to the real Secret Service. Run with:
//! cargo test -p opal-core --test keyring -- --ignored

use nostr::key::Keys;
use opal_core::keystore::{ItemKind, SecretStore};
use opal_core::vault::Vault;

#[tokio::test]
#[ignore = "needs a running Secret Service"]
async fn real_keyring_roundtrip() {
    let vault = Vault::with_log_n(SecretStore::keyring().await.unwrap(), 8);
    let keys = Keys::generate();
    let pk = vault.add_account(keys, "integration-test").await.unwrap();

    let stored = vault
        .store()
        .get(ItemKind::Account, &pk.to_hex())
        .await
        .unwrap()
        .expect("item stored");
    assert!(
        stored.starts_with("ncryptsec1"),
        "only ncryptsec is written"
    );

    vault.unlock("integration-test").await.unwrap();
    assert_eq!(vault.keys(&pk).await.unwrap().public_key(), pk);

    vault.remove_account(&pk).await.unwrap();
    assert!(
        vault
            .store()
            .get(ItemKind::Account, &pk.to_hex())
            .await
            .unwrap()
            .is_none()
    );
}

/// The store on its own, without the vault: every operation goes to the Secret
/// Service with no passphrase involved, so it runs against a keyring that
/// already holds accounts. The item is removed again at the end.
#[tokio::test]
#[ignore = "needs a running Secret Service"]
async fn real_store_roundtrip() {
    let store = SecretStore::keyring().await.unwrap();
    let id = format!("test-{}", Keys::generate().public_key().to_hex());

    store
        .put(ItemKind::ConnKey, &id, "opal integration test", "not-a-key")
        .await
        .unwrap();
    assert_eq!(
        store
            .get(ItemKind::ConnKey, &id)
            .await
            .unwrap()
            .map(|s| s.to_string())
            .as_deref(),
        Some("not-a-key")
    );
    assert!(
        store
            .list(ItemKind::ConnKey)
            .await
            .unwrap()
            .iter()
            .any(|(k, _)| k == &id),
        "the new item is listed"
    );

    store.delete(ItemKind::ConnKey, &id).await.unwrap();
    assert!(
        store.get(ItemKind::ConnKey, &id).await.unwrap().is_none(),
        "deleted"
    );
    assert!(
        !store
            .list(ItemKind::ConnKey)
            .await
            .unwrap()
            .iter()
            .any(|(k, _)| k == &id)
    );

    // The keyring is still readable afterwards: the client is still live.
    assert!(!store.is_locked().await.unwrap());
}
