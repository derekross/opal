//! Event templates: NIP-38 statuses (kind 30315) and scrobbles (kind 1073,
//! see docs/nip-scrobble.md).

use nostr_sdk::prelude::*;

use crate::mpris::Track;

pub const KIND_STATUS: u16 = 30315;
pub const KIND_SCROBBLE: u16 = 1073;

/// A user status. Empty `content` clears it.
pub fn status(d: &str, content: &str, link: Option<&str>, expires_at: Option<u64>) -> EventBuilder {
    let mut tags = vec![Tag::identifier(d)];
    if let Some(l) = link.filter(|l| !l.is_empty()) {
        tags.push(Tag::parse(["r", l]).expect("r tag"));
    }
    if let Some(t) = expires_at {
        tags.push(Tag::expiration(Timestamp::from(t)));
    }
    EventBuilder::new(Kind::from(KIND_STATUS), content).tags(tags)
}

/// Clearing: empty content that also expires soon, so relays can drop it.
pub fn clear(d: &str, now: u64) -> EventBuilder {
    status(d, "", None, Some(now + 3600))
}

/// One listen.
pub fn scrobble(t: &Track, link: Option<&str>, played_at: u64) -> EventBuilder {
    let mut tags = vec![
        Tag::parse(["title", &t.title]).expect("tag"),
        Tag::parse(["alt", &format!("Listened to {}", t.display())]).expect("tag"),
    ];
    for artist in t.artist.split(", ").filter(|a| !a.trim().is_empty()) {
        tags.push(Tag::parse(["artist", artist.trim()]).expect("tag"));
    }
    if let Some(album) = &t.album {
        tags.push(Tag::parse(["album", album]).expect("tag"));
    }
    if let Some(len) = t.length {
        tags.push(Tag::parse(["duration", &len.as_secs().to_string()]).expect("tag"));
    }
    if let Some(l) = link {
        tags.push(Tag::parse(["r", l]).expect("tag"));
    }
    EventBuilder::new(Kind::from(KIND_SCROBBLE), "")
        .tags(tags)
        .custom_created_at(Timestamp::from(played_at))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn tags(ev: &Event) -> Vec<Vec<String>> {
        ev.tags.iter().map(|t| t.as_slice().to_vec()).collect()
    }

    #[test]
    fn status_shape() {
        let keys = Keys::generate();
        let ev = status(
            "music",
            "Artist - Song",
            Some("https://x.example"),
            Some(123),
        )
        .finalize(&keys)
        .unwrap();
        assert_eq!(ev.kind.as_u16(), 30315);
        assert_eq!(ev.content, "Artist - Song");
        let t = tags(&ev);
        assert!(t.contains(&vec!["d".into(), "music".into()]));
        assert!(t.contains(&vec!["r".into(), "https://x.example".into()]));
        assert!(t.contains(&vec!["expiration".into(), "123".into()]));

        let cleared = clear("general", 1000).finalize(&keys).unwrap();
        assert_eq!(cleared.content, "");
        assert!(tags(&cleared).contains(&vec!["expiration".into(), "4600".into()]));
    }

    #[test]
    fn scrobble_shape() {
        let keys = Keys::generate();
        let t = Track {
            player: "spotify".into(),
            title: "Schism".into(),
            artist: "Tool, Maynard".into(),
            album: Some("Lateralus".into()),
            length: Some(Duration::from_secs(407)),
            position: None,
            url: None,
            track_id: None,
            art_url: None,
            playing: true,
        };
        let ev = scrobble(&t, None, 1_700_000_000).finalize(&keys).unwrap();
        assert_eq!(ev.kind.as_u16(), 1073);
        assert_eq!(ev.created_at.as_secs(), 1_700_000_000);
        let t = tags(&ev);
        assert!(t.contains(&vec!["title".into(), "Schism".into()]));
        assert!(t.contains(&vec!["artist".into(), "Tool".into()]));
        assert!(t.contains(&vec!["artist".into(), "Maynard".into()]));
        assert!(t.contains(&vec!["album".into(), "Lateralus".into()]));
        assert!(t.contains(&vec!["duration".into(), "407".into()]));
    }
}
