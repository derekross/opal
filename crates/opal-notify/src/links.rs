//! Deep links that open a notification in a web client.

use nostr_sdk::prelude::*;

/// The client id that opens notifications in the desktop's default Nostr
/// app (a `nostr:` or `web+nostr:` link) instead of a web client.
pub const DEFAULT_APP: &str = "default";

/// Where "Default app" goes when no app on the computer opens Nostr links.
pub const DEFAULT_APP_FALLBACK: &str = "https://njump.me/";

/// (id, label, base URL the nevent is appended to). "Default app" has no
/// base URL: the caller picks a scheme, see [`Opener`].
pub const CLIENTS: &[(&str, &str, &str)] = &[
    (DEFAULT_APP, "Default app", ""),
    ("primal", "Primal", "https://primal.net/e/"),
    ("jumble", "Jumble", "https://jumble.social/notes/"),
    ("coracle", "Coracle", "https://coracle.social/notes/"),
    ("nostrudel", "noStrudel", "https://nostrudel.ninja/#/n/"),
    ("ditto", "Ditto", "https://ditto.pub/"),
    ("nostrich", "Nostrich", "https://nostrich.org/e/"),
    ("njump", "njump", "https://njump.me/"),
];

/// How a link starts: a web client's base URL, or a URI scheme the
/// desktop hands to whichever app is registered for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opener {
    Web(&'static str),
    Scheme(&'static str),
}

/// The web client `client` names; unknown ids (and "Default app", which the
/// caller resolves itself) get the first real web client.
pub fn client_base(client: &str) -> &'static str {
    CLIENTS
        .iter()
        .find(|(id, _, base)| *id == client && !base.is_empty())
        .or_else(|| CLIENTS.iter().find(|(_, _, base)| !base.is_empty()))
        .map(|(_, _, base)| *base)
        .unwrap_or(DEFAULT_APP_FALLBACK)
}

/// Link to `event_id` in the web client `client`.
pub fn event_url(
    client: &str,
    event_id: &str,
    author: Option<&str>,
    kind: Option<u16>,
    relay: Option<&str>,
) -> Option<String> {
    event_link(
        Opener::Web(client_base(client)),
        event_id,
        author,
        kind,
        relay,
    )
}

/// Link to `event_id` with the hints a client needs to find it.
pub fn event_link(
    opener: Opener,
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
    let nevent = nev.to_bech32().ok()?;
    Some(match opener {
        Opener::Web(base) => format!("{base}{nevent}"),
        Opener::Scheme(scheme) => format!("{scheme}:{nevent}"),
    })
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
        // "Default app" is resolved by the caller; asked directly, it gets
        // a web client rather than an empty base.
        assert!(
            event_url(DEFAULT_APP, &id, None, None, None)
                .unwrap()
                .starts_with("https://primal.net/e/")
        );
    }

    #[test]
    fn builds_scheme_links() {
        let id = "b".repeat(64);
        for scheme in ["nostr", "web+nostr"] {
            let url = event_link(Opener::Scheme(scheme), &id, None, Some(1), None).unwrap();
            let body = url.strip_prefix(&format!("{scheme}:")).unwrap();
            assert!(body.starts_with("nevent1"), "{url}");
            assert_eq!(Nip19Event::from_bech32(body).unwrap().event_id.to_hex(), id);
        }
    }
}
