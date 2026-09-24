//! Deep links that open a notification in a web client.

use nostr_sdk::prelude::*;

/// (id, label, base URL the nevent is appended to)
pub const CLIENTS: &[(&str, &str, &str)] = &[
    ("primal", "Primal", "https://primal.net/e/"),
    ("jumble", "Jumble", "https://jumble.social/notes/"),
    ("coracle", "Coracle", "https://coracle.social/notes/"),
    ("ditto", "Ditto", "https://ditto.pub/"),
    ("njump", "njump", "https://njump.me/"),
];

pub fn client_base(client: &str) -> &'static str {
    CLIENTS
        .iter()
        .find(|(id, ..)| *id == client)
        .map(|(_, _, base)| *base)
        .unwrap_or(CLIENTS[0].2)
}

/// Link to `event_id` with the hints a client needs to find it.
pub fn event_url(
    client: &str,
    event_id: &str,
    author: Option<&str>,
    kind: Option<u16>,
    relay: Option<&str>,
) -> Option<String> {
    let id = EventId::from_hex(event_id).ok()?;
    let mut nev = Nip19Event::new(id);
    if let Some(pk) = author.and_then(|a| PublicKey::from_hex(a).ok()) {
        nev = nev.author(pk);
    }
    if let Some(k) = kind {
        nev = nev.kind(Kind::from(k));
    }
    if let Some(r) = relay.and_then(|r| RelayUrl::parse(r).ok()) {
        nev = nev.relays([r]);
    }
    Some(format!("{}{}", client_base(client), nev.to_bech32().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_nevent_links() {
        let id = "a".repeat(64);
        let url = event_url(
            "jumble",
            &id,
            None,
            Some(1),
            Some("wss://relay.example.com"),
        )
        .unwrap();
        assert!(
            url.starts_with("https://jumble.social/notes/nevent1"),
            "{url}"
        );
        let parsed =
            Nip19Event::from_bech32(url.trim_start_matches("https://jumble.social/notes/"))
                .unwrap();
        assert_eq!(parsed.event_id.to_hex(), id);
        assert_eq!(parsed.relays.len(), 1);
        assert!(
            event_url("unknown", &id, None, None, None)
                .unwrap()
                .starts_with("https://primal.net/e/")
        );
        assert!(event_url("primal", "nope", None, None, None).is_none());
    }
}
