//! Turning events that tag you into notifications.

use nostr_sdk::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotifType {
    Reply,
    Mention,
    Repost,
    Reaction,
    Zap,
    Dm,
}

impl NotifType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reply => "reply",
            Self::Mention => "mention",
            Self::Repost => "repost",
            Self::Reaction => "reaction",
            Self::Zap => "zap",
            Self::Dm => "dm",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "reply" => Self::Reply,
            "mention" => Self::Mention,
            "repost" => Self::Repost,
            "reaction" => Self::Reaction,
            "zap" => Self::Zap,
            "dm" => Self::Dm,
            _ => return None,
        })
    }
}

/// A notification before it is stored.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Notification {
    /// Event id (for DMs: the gift wrap id).
    pub id: String,
    #[serde(rename = "type")]
    pub ntype: NotifType,
    pub kind: u16,
    /// Who did it (for zaps: the zapper, not the LNURL server).
    pub author: String,
    pub created_at: u64,
    /// Short text: the reply, the reaction, the zap message.
    pub detail: String,
    /// The event this one refers to (your note that was replied to, …).
    pub ref_id: Option<String>,
    pub sats: Option<u64>,
    /// First image or video URL in the content.
    pub media: Option<String>,
    /// Relay it arrived on (a hint for links).
    pub relay: Option<String>,
}

const DETAIL_MAX: usize = 280;

/// `None` if the event isn't something we notify about.
pub fn classify(ev: &Event, me: &PublicKey) -> Option<Notification> {
    let kind = ev.kind.as_u16();
    let ntype = match kind {
        9735 => NotifType::Zap,
        6 | 16 => NotifType::Repost,
        7 => NotifType::Reaction,
        1 => {
            if has_tag(ev, "e") {
                NotifType::Reply
            } else {
                NotifType::Mention
            }
        }
        1111 => NotifType::Reply,
        _ => return None,
    };
    // It has to be addressed to you.
    if !ev.tags.public_keys().any(|pk| pk == *me) {
        return None;
    }
    // Your own activity isn't news. Zap receipts are signed by the LNURL
    // server; the zapper is whoever signed the zap request inside, which
    // must check out or the "zap" is ignored.
    let author = match ntype {
        NotifType::Zap => valid_zap_sender(ev, me)?,
        _ => ev.pubkey,
    };
    if author == *me {
        return None;
    }

    let (detail, media, sats) = match ntype {
        NotifType::Zap => {
            let sats = zap_sats(ev);
            let msg = zap_message(ev);
            (truncate(&msg, DETAIL_MAX), None, sats)
        }
        NotifType::Reaction => (reaction_display(&ev.content), None, None),
        NotifType::Repost => (String::new(), None, None),
        _ => {
            let media = first_media_url(&ev.content);
            let text = match &media {
                Some(url) => ev.content.replace(url, " "),
                None => ev.content.clone(),
            };
            (
                truncate(&collapse(&tidy_refs(&text)), DETAIL_MAX),
                media,
                None,
            )
        }
    };

    Some(Notification {
        id: ev.id.to_hex(),
        ntype,
        kind,
        author: author.to_hex(),
        created_at: ev.created_at.as_secs(),
        detail,
        ref_id: referenced_event(ev),
        sats,
        media,
        relay: None,
    })
}

fn has_tag(ev: &Event, name: &str) -> bool {
    ev.tags
        .iter()
        .any(|t| t.as_slice().first().map(String::as_str) == Some(name))
}

/// NIP-10: prefer the `reply` marker, then `root`, then the last `e` tag.
/// NIP-22 comments may only carry the uppercase root `E`.
fn referenced_event(ev: &Event) -> Option<String> {
    let mut reply = None;
    let mut root = None;
    let mut last = None;
    let mut upper = None;
    for t in ev.tags.iter() {
        let s = t.as_slice();
        let (Some(name), Some(id)) = (s.first(), s.get(1)) else {
            continue;
        };
        if EventId::from_hex(id).is_err() {
            continue;
        }
        match name.as_str() {
            "e" => {
                match s.get(3).map(String::as_str) {
                    Some("reply") => reply = Some(id.clone()),
                    Some("root") => root = Some(id.clone()),
                    _ => {}
                }
                last = Some(id.clone());
            }
            "E" if upper.is_none() => upper = Some(id.clone()),
            _ => {}
        }
    }
    reply.or(root).or(last).or(upper).map(|s| s.to_lowercase())
}

/// The zap request (kind 9734) lives in the receipt's `description` tag.
fn zap_request(ev: &Event) -> Option<serde_json::Value> {
    let desc = ev
        .tags
        .iter()
        .find(|t| t.as_slice().first().map(String::as_str) == Some("description"))
        .and_then(|t| t.as_slice().get(1).cloned())?;
    serde_json::from_str(&desc).ok()
}

/// NIP-57 checks we can do without contacting the recipient's wallet: the
/// embedded zap request is a validly signed kind 9734, it is for you, and
/// its amount (when given) matches the invoice. Otherwise anyone could
/// publish "someone zapped you a million sats".
fn valid_zap_sender(ev: &Event, me: &PublicKey) -> Option<PublicKey> {
    let desc = ev
        .tags
        .iter()
        .find(|t| t.as_slice().first().map(String::as_str) == Some("description"))
        .and_then(|t| t.as_slice().get(1).cloned())?;
    let req = Event::from_json(&desc).ok()?;
    if req.kind != Kind::ZapRequest || req.verify().is_err() {
        return None;
    }
    if !req.tags.public_keys().any(|pk| pk == *me) {
        return None;
    }
    let requested_msats = req.tags.iter().find_map(|t| {
        let s = t.as_slice();
        (s.first().map(String::as_str) == Some("amount"))
            .then(|| s.get(1)?.parse::<u64>().ok())
            .flatten()
    });
    if let Some(msats) = requested_msats {
        let invoiced = ev.tags.iter().find_map(|t| {
            let s = t.as_slice();
            (s.first().map(String::as_str) == Some("bolt11"))
                .then(|| s.get(1).and_then(|b| bolt11_sats(b)))
                .flatten()
        });
        if invoiced.is_some_and(|sats| sats != msats / 1000) {
            return None;
        }
    }
    Some(req.pubkey)
}

fn zap_message(ev: &Event) -> String {
    zap_request(ev)
        .and_then(|r| r.get("content").and_then(|c| c.as_str()).map(collapse))
        .unwrap_or_default()
}

fn zap_sats(ev: &Event) -> Option<u64> {
    let bolt11 = ev
        .tags
        .iter()
        .find(|t| t.as_slice().first().map(String::as_str) == Some("bolt11"))
        .and_then(|t| t.as_slice().get(1).cloned());
    if let Some(sats) = bolt11.as_deref().and_then(bolt11_sats) {
        return Some(sats);
    }
    // Fall back to the requested amount (millisats) in the zap request.
    let req = zap_request(ev)?;
    let tags = req.get("tags")?.as_array()?;
    tags.iter()
        .filter_map(|t| t.as_array())
        .find(|t| t.first().and_then(|v| v.as_str()) == Some("amount"))
        .and_then(|t| t.get(1)?.as_str()?.parse::<u64>().ok())
        .map(|msats| msats / 1000)
}

/// Amount from a BOLT11 invoice's human-readable part (`lnbc2500u1…`).
pub fn bolt11_sats(invoice: &str) -> Option<u64> {
    let lower = invoice.to_ascii_lowercase();
    let rest = ["lnbcrt", "lntbs", "lnbc", "lntb"]
        .iter()
        .find_map(|p| lower.strip_prefix(p))?;
    let hrp = &rest[..rest.rfind('1')?];
    let digits: String = hrp.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let amount: u64 = digits.parse().ok()?;
    let unit = hrp[digits.len()..].chars().next();
    // Amount is in BTC times the multiplier; 1 BTC = 1e8 sats = 1e11 msat.
    let msats = match unit {
        None => amount.checked_mul(100_000_000_000)?,
        Some('m') => amount.checked_mul(100_000_000)?,
        Some('u') => amount.checked_mul(100_000)?,
        Some('n') => amount.checked_mul(100)?,
        Some('p') => amount / 10,
        Some(_) => return None,
    };
    Some(msats / 1000)
}

fn reaction_display(content: &str) -> String {
    let c = collapse(content);
    match c.as_str() {
        "" | "+" => "❤️".into(),
        "-" => "👎".into(),
        _ => truncate(&c, 24),
    }
}

fn first_media_url(content: &str) -> Option<String> {
    const EXT: &[&str] = &[
        ".png", ".jpg", ".jpeg", ".gif", ".webp", ".avif", ".mp4", ".webm", ".mov",
    ];
    content
        .split_whitespace()
        .filter(|w| w.starts_with("https://"))
        .map(|w| w.trim_end_matches(['.', ',', ')', ';', '!', '?']))
        .find(|u| {
            let path = u.split(['?', '#']).next().unwrap_or(u).to_ascii_lowercase();
            EXT.iter().any(|e| path.ends_with(e))
        })
        .map(String::from)
}

/// Shorten NIP-21 references: `nostr:npub1…`/`nprofile1…` become `@npub1abcd…`,
/// event references become `[note]`.
fn tidy_refs(s: &str) -> String {
    s.split_inclusive(char::is_whitespace)
        .map(|word| {
            // Keep trailing whitespace and punctuation outside the reference.
            let trimmed = word
                .trim_end()
                .trim_end_matches(['.', ',', ';', ':', '!', '?', ')']);
            let tail = &word[trimmed.len()..];
            let bare = trimmed.strip_prefix("nostr:").unwrap_or(trimmed);
            if !trimmed.starts_with("nostr:")
                && !bare.starts_with("npub1")
                && !bare.starts_with("nprofile1")
            {
                return word.to_string();
            }
            let short = if bare.starts_with("npub1") || bare.starts_with("nprofile1") {
                match Nip19Profile::from_bech32(bare)
                    .map(|p| p.public_key)
                    .or_else(|_| PublicKey::from_bech32(bare))
                {
                    Ok(pk) => {
                        let npub = pk.to_bech32().unwrap_or_default();
                        format!("@{}…", &npub[..npub.len().min(12)])
                    }
                    Err(_) => trimmed.to_string(),
                }
            } else if ["note1", "nevent1", "naddr1"]
                .iter()
                .any(|p| bare.starts_with(p))
            {
                "[note]".to_string()
            } else {
                trimmed.to_string()
            };
            format!("{short}{tail}")
        })
        .collect()
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(keys: &Keys, kind: u16, content: &str, tags: Vec<Vec<&str>>) -> Event {
        let tags = tags
            .into_iter()
            .map(|t| Tag::parse(t).unwrap())
            .collect::<Vec<_>>();
        EventBuilder::new(Kind::from(kind), content)
            .tags(tags)
            .finalize(keys)
            .unwrap()
    }

    #[test]
    fn replies_and_mentions() {
        let me = Keys::generate().public_key();
        let them = Keys::generate();
        let root = "0".repeat(64);
        let parent = "a".repeat(64);
        let reply = event(
            &them,
            1,
            "nice  post https://img.example/a.png",
            vec![
                vec!["e", &root, "", "root"],
                vec!["e", &parent, "", "reply"],
                vec!["p", &me.to_hex()],
            ],
        );
        let n = classify(&reply, &me).unwrap();
        assert_eq!(n.ntype, NotifType::Reply);
        assert_eq!(n.ref_id.as_deref(), Some(parent.as_str()));
        assert_eq!(n.media.as_deref(), Some("https://img.example/a.png"));
        assert_eq!(n.detail, "nice post");

        let mention = event(&them, 1, "hey", vec![vec!["p", &me.to_hex()]]);
        assert_eq!(classify(&mention, &me).unwrap().ntype, NotifType::Mention);
    }

    #[test]
    fn own_events_are_ignored() {
        let me = Keys::generate();
        let ev = event(
            &me,
            1,
            "talking to myself",
            vec![vec!["p", &me.public_key().to_hex()]],
        );
        assert!(classify(&ev, &me.public_key()).is_none());
    }

    #[test]
    fn reactions() {
        let me = Keys::generate().public_key();
        let them = Keys::generate();
        let id = "b".repeat(64);
        let like = event(&them, 7, "+", vec![vec!["e", &id], vec!["p", &me.to_hex()]]);
        let n = classify(&like, &me).unwrap();
        assert_eq!((n.ntype, n.detail.as_str()), (NotifType::Reaction, "❤️"));
        assert_eq!(n.ref_id.as_deref(), Some(id.as_str()));
    }

    #[test]
    fn zaps_credit_the_zapper() {
        let me = Keys::generate().public_key();
        let zapper = Keys::generate();
        let lnurl_server = Keys::generate();
        let request = event(
            &zapper,
            9734,
            "great talk",
            vec![vec!["p", &me.to_hex()], vec!["amount", "21000"]],
        );
        let receipt = event(
            &lnurl_server,
            9735,
            "",
            vec![
                vec!["p", &me.to_hex()],
                vec!["bolt11", "lnbc210n1pjabcdef"],
                vec!["description", &request.as_json()],
            ],
        );
        let n = classify(&receipt, &me).unwrap();
        assert_eq!(n.ntype, NotifType::Zap);
        assert_eq!(n.author, zapper.public_key().to_hex());
        assert_eq!(n.sats, Some(21));
        assert_eq!(n.detail, "great talk");
    }

    #[test]
    fn forged_zaps_are_ignored() {
        let me = Keys::generate().public_key();
        let someone = Keys::generate();
        let server = Keys::generate();
        let receipt = |desc: String, bolt11: &str| {
            event(
                &server,
                9735,
                "",
                vec![
                    vec!["p", &me.to_hex()],
                    vec!["bolt11", bolt11],
                    vec!["description", &desc],
                ],
            )
        };
        // Request for someone else.
        let other = Keys::generate().public_key();
        let req = event(&someone, 9734, "", vec![vec!["p", &other.to_hex()]]);
        assert!(classify(&receipt(req.as_json(), "lnbc210n1x"), &me).is_none());
        // Tampered request (signature no longer matches).
        let req = event(&someone, 9734, "hi", vec![vec!["p", &me.to_hex()]]);
        let tampered = req.as_json().replace("\"hi\"", "\"send me your seed\"");
        assert!(classify(&receipt(tampered, "lnbc210n1x"), &me).is_none());
        // Asked for 21 sats, invoice says 1,000,000.
        let req = event(
            &someone,
            9734,
            "",
            vec![vec!["p", &me.to_hex()], vec!["amount", "21000"]],
        );
        assert!(classify(&receipt(req.as_json(), "lnbc10m1x"), &me).is_none());
        // Not a zap request at all.
        let note = event(&someone, 1, "", vec![vec!["p", &me.to_hex()]]);
        assert!(classify(&receipt(note.as_json(), "lnbc210n1x"), &me).is_none());
    }

    #[test]
    fn events_not_addressed_to_me_are_ignored() {
        let me = Keys::generate().public_key();
        let ev = event(&Keys::generate(), 1, "hello", vec![]);
        assert!(classify(&ev, &me).is_none());
    }

    #[test]
    fn bolt11_amounts() {
        assert_eq!(bolt11_sats("lnbc2500u1pvjluez"), Some(250_000));
        assert_eq!(bolt11_sats("lnbc1m1xyz"), Some(100_000));
        assert_eq!(bolt11_sats("lnbc210n1xyz"), Some(21));
        assert_eq!(bolt11_sats("lnbc10p1xyz"), Some(0));
        assert_eq!(bolt11_sats("lnbc1pvjluez"), None, "no amount");
        assert_eq!(bolt11_sats("garbage"), None);
    }

    #[test]
    fn nostr_references_are_shortened() {
        let pk = Keys::generate().public_key();
        let npub = pk.to_bech32().unwrap();
        let prof = Nip19Profile::new(pk, []).to_bech32().unwrap();
        let out = tidy_refs(&format!("hi nostr:{prof} and {npub}, see nostr:note1abc"));
        assert_eq!(
            out,
            format!("hi @{}… and @{}…, see [note]", &npub[..12], &npub[..12])
        );
    }

    #[test]
    fn unrelated_kinds_are_skipped() {
        let me = Keys::generate().public_key();
        let ev = event(
            &Keys::generate(),
            30023,
            "article",
            vec![vec!["p", &me.to_hex()]],
        );
        assert!(classify(&ev, &me).is_none());
    }
}
