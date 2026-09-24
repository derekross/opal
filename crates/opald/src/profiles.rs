//! Fetching kind 0 profiles for our accounts (names and avatars in the UI).

use std::sync::Arc;
use std::time::Duration;

use nostr_sdk::prelude::*;
use opal_core::accounts::Profile;
use serde_json::{Value, json};

use crate::app::{App, parse_relays};

const FETCH_TIMEOUT: Duration = Duration::from_secs(8);
/// Don't refetch more often than this.
const REFRESH_AFTER_SECS: u64 = 6 * 3600;

pub async fn refresh_all(app: &Arc<App>) {
    let stale: Vec<PublicKey> = app
        .accounts
        .list()
        .unwrap_or_default()
        .into_iter()
        .filter(|a| {
            a.fetched_at
                .is_none_or(|t| Timestamp::now().as_secs().saturating_sub(t) > REFRESH_AFTER_SECS)
        })
        .filter_map(|a| PublicKey::from_hex(&a.pubkey).ok())
        .collect();
    if !stale.is_empty() {
        refresh(app, &stale).await;
    }
}

pub async fn refresh(app: &Arc<App>, pks: &[PublicKey]) {
    if !app.is_online() {
        return;
    }
    let relays = parse_relays(&app.config.read().await.signer.profile_relays);
    let client = Client::default();
    for r in &relays {
        let _ = client.add_relay(r).await;
    }
    client.connect().and_wait(Duration::from_secs(4)).await;
    let filter = Filter::new().kind(Kind::Metadata).authors(pks.to_vec());
    let events = match client.fetch_events(filter).timeout(FETCH_TIMEOUT).await {
        Ok(e) => e,
        Err(e) => {
            tracing::debug!("profile fetch failed: {e}");
            client.shutdown().await;
            return;
        }
    };
    client.shutdown().await;

    let now = Timestamp::now().as_secs();
    let mut changed = false;
    for pk in pks {
        // Newest kind 0 per author wins.
        let Some(ev) = events
            .iter()
            .filter(|e| e.pubkey == *pk)
            .max_by_key(|e| e.created_at)
        else {
            continue;
        };
        let profile = parse_profile(&ev.content);
        if app.accounts.set_profile(pk, &profile, now).is_ok() {
            changed = true;
        }
    }
    if changed {
        app.emit_state().await;
    }
}

fn parse_profile(content: &str) -> Profile {
    let v: Value = serde_json::from_str(content).unwrap_or(json!({}));
    let s = |k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
    };
    Profile {
        name: s("name"),
        display_name: s("display_name").or_else(|| s("displayName")),
        picture: s("picture").filter(|p| p.starts_with("https://")),
        nip05: s("nip05"),
        about: s("about").map(|a| a.chars().take(280).collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_profiles_defensively() {
        let p = parse_profile(
            r#"{"name":" derek ","displayName":"Derek","picture":"http://insecure/x.png","about":""}"#,
        );
        assert_eq!(p.name.as_deref(), Some("derek"));
        assert_eq!(p.display_name.as_deref(), Some("Derek"));
        assert_eq!(p.picture, None, "only https avatars");
        assert_eq!(p.about, None);
        assert_eq!(parse_profile("not json"), Profile::default());
    }
}
