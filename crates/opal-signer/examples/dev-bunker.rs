//! Throwaway bunker for interop testing: starts an in-process relay and a
//! signer that approves everything, then prints one line of JSON:
//! `{"relay": "...", "bunker": "...", "pubkey": "..."}`.
//! Any `nostrconnect://` URI written to stdin is accepted.
//!
//! cargo run -p opal-signer --example dev-bunker

use std::sync::Arc;
use std::time::Duration;

use nostr_sdk::prelude::*;
use opal_core::config::Policy;
use opal_core::keystore::SecretStore;
use opal_core::vault::Vault;
use opal_signer::{AllowAll, Signer, SignerSettings};

#[tokio::main]
async fn main() {
    let relay = MockRelay::run().await.expect("relay");
    let relay_url = relay.url().await;
    let vault = Arc::new(Vault::with_log_n(SecretStore::memory(), 4));
    let account = vault.add_account(Keys::generate(), "dev").await.unwrap();
    vault.unlock("dev").await.unwrap();
    let signer = Signer::new(
        vault,
        Arc::new(AllowAll),
        SignerSettings {
            default_relays: vec![relay_url.clone()],
            pending_timeout: Duration::from_secs(30),
            log_activity: false,
        },
    );
    signer.start().await.unwrap();
    let (_, uri) = signer
        .create_bunker(
            account,
            Some("interop".into()),
            None,
            Policy::FullTrust,
            None,
        )
        .await
        .unwrap();
    println!(
        "{}",
        serde_json::json!({"relay": relay_url.to_string(), "bunker": uri, "pubkey": account.to_hex()})
    );
    use tokio::io::AsyncBufReadExt;
    let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    let deadline = tokio::time::sleep(Duration::from_secs(120));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => break,
            line = lines.next_line() => match line {
                Ok(Some(l)) if l.starts_with("nostrconnect://") => {
                    let parsed = opal_signer::NostrConnectUri::parse(&l).expect("valid nostrconnect uri");
                    signer.accept_nostrconnect(&parsed, account, Policy::FullTrust, &[]).await.unwrap();
                    println!("{}", serde_json::json!({"accepted": parsed.name}));
                }
                Ok(Some(_)) => {}
                _ => break,
            }
        }
    }
}
