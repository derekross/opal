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
