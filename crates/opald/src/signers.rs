//! How Opal's own modules sign: with the local vault, or through an external
//! NIP-46 bunker (e.g. Amber on your phone). The signers live in opal-kit;
//! this adds Opal's activity log.

use std::sync::Arc;

use nostr_sdk::prelude::*;
use opal_kit::signer::OnSign;
pub use opal_kit::signer::{BunkerSigner, LocalSigner};
use opal_signer::SignerStore;
use opal_signer::permissions::Source;
use opal_signer::protocol::Method;
use opal_signer::store::ActivityEntry;

/// Log each signature made by Opal's own features as "Opal Status".
pub fn log_as_opal_status(log: Option<SignerStore>) -> Option<OnSign> {
    let log = log?;
    Some(Arc::new(move |account: &PublicKey, kind: u16| {
        let _ = log.log_activity(&ActivityEntry {
            id: 0,
            at: Timestamp::now().as_secs(),
            app_id: "opal-status".into(),
            app_name: "Opal Status".into(),
            account: account.to_hex(),
            method: Method::SignEvent,
            kind: Some(kind),
            kind_label: None,
            allowed: true,
            source: Source::Automatic,
            reason: None,
        });
    }))
}
