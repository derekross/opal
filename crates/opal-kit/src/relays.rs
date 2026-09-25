//! Talking to relays: finding someone's relay list (NIP-65), publishing, a
//! persistent outbox for events that couldn't be sent yet, and asking a
//! relay what it supports (NIP-11).

use std::time::Duration;

use nostr_sdk::prelude::*;
use opal_core::db::Db;
use serde::Deserialize;

/// How long to wait for relays to connect before carrying on.
pub const CONNECT_WAIT: Duration = Duration::from_secs(5);

/// A user's NIP-65 relay list, split by direction. Relays without a marker
/// count as both.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RelayList {
    pub read: Vec<RelayUrl>,
    pub write: Vec<RelayUrl>,
}

impl RelayList {
    pub fn from_event(ev: &Event) -> Self {
        let mut list = Self::default();
        for (url, meta) in nip65::extract_relay_list(ev) {
            if meta.is_none() || meta == Some(RelayMetadata::Read) {
                list.read.push(url.clone());
            }
            if meta.is_none() || meta == Some(RelayMetadata::Write) {
                list.write.push(url.clone());
            }
        }
        list
    }
}

pub fn parse_urls<S: AsRef<str>>(urls: &[S]) -> Vec<RelayUrl> {
    urls.iter()
        .filter_map(|r| RelayUrl::parse(r.as_ref()).ok())
        .collect()
}

/// Fetch `who`'s newest relay list from `bootstrap` relays (which are added
/// to `client`). None if no relay had one within `timeout`.
pub async fn fetch_relay_list(
    client: &Client,
    bootstrap: &[RelayUrl],
    who: PublicKey,
    timeout: Duration,
) -> Option<RelayList> {
    for r in bootstrap {
        let _ = client.add_relay(r).await;
    }
    client.connect().and_wait(CONNECT_WAIT).await;
    let filter = Filter::new().author(who).kind(Kind::RelayList);
    let targets: Vec<(RelayUrl, Vec<Filter>)> = bootstrap
        .iter()
        .map(|r| (r.clone(), vec![filter.clone()]))
        .collect();
    let events = client.fetch_events(targets).timeout(timeout).await.ok()?;
    events
        .iter()
        .max_by_key(|e| e.created_at)
        .map(RelayList::from_event)
}

/// Send `ev` to `relays` (already added to `client`). Ok with the number of
/// relays that accepted it, or why none did.
pub async fn publish(client: &Client, ev: &Event, relays: &[RelayUrl]) -> Result<usize, String> {
    match client.send_event(ev).to(relays.to_vec()).await {
        Ok(out) if !out.success.is_empty() => Ok(out.success.len()),
        Ok(out) => Err(format!(
            "no relay accepted it: {}",
            out.failed.values().next().cloned().unwrap_or_default()
        )),
        Err(e) => Err(e.to_string()),
    }
}

/// Events waiting to be sent, kept in the database so they survive a
/// restart. Newer events for the same replaceable/addressable coordinate
/// replace older ones.
#[derive(Clone)]
pub struct Outbox {
    db: Db,
}

/// Longest wait between retries.
const MAX_BACKOFF: u64 = 30 * 60;

impl Outbox {
    pub fn new(db: Db) -> opal_core::Result<Self> {
        db.migrate(
            "kit.outbox",
            &["CREATE TABLE kit_outbox (
                id TEXT PRIMARY KEY,
                coordinate TEXT,
                event TEXT NOT NULL,
                attempts INTEGER NOT NULL DEFAULT 0,
                next_at INTEGER NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE UNIQUE INDEX kit_outbox_coordinate ON kit_outbox(coordinate)
                WHERE coordinate IS NOT NULL;"],
        )?;
        Ok(Self { db })
    }

    /// Queue `ev` to be sent as soon as possible.
    pub fn push(&self, ev: &Event) -> opal_core::Result<()> {
        let coordinate = coordinate(ev);
        let json = ev.as_json();
        let id = ev.id.to_hex();
        let now = Timestamp::now().as_secs() as i64;
        self.db.with(|c| {
            if let Some(coord) = &coordinate {
                c.execute("DELETE FROM kit_outbox WHERE coordinate = ?1", [coord])?;
            }
            c.execute(
                "INSERT OR REPLACE INTO kit_outbox (id, coordinate, event, next_at, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?4)",
                rusqlite::params![id, coordinate, json, now],
            )?;
            Ok(())
        })
    }

    /// Events whose retry time has come, oldest first.
    pub fn due(&self, limit: usize) -> opal_core::Result<Vec<Event>> {
        let now = Timestamp::now().as_secs() as i64;
        let rows: Vec<String> = self.db.with(|c| {
            let mut st = c.prepare(
                "SELECT event FROM kit_outbox WHERE next_at <= ?1 ORDER BY created_at LIMIT ?2",
            )?;
            st.query_map(rusqlite::params![now, limit as i64], |r| r.get(0))?
                .collect()
        })?;
        Ok(rows
            .iter()
            .filter_map(|j| Event::from_json(j).ok())
            .collect())
    }

    pub fn sent(&self, id: &EventId) -> opal_core::Result<()> {
        self.db.with(|c| {
            c.execute("DELETE FROM kit_outbox WHERE id = ?1", [id.to_hex()])?;
            Ok(())
        })
    }

    /// Try again later, backing off from 30 s up to 30 minutes.
    pub fn failed(&self, id: &EventId) -> opal_core::Result<()> {
        let now = Timestamp::now().as_secs() as i64;
        self.db.with(|c| {
            c.execute(
                "UPDATE kit_outbox SET attempts = attempts + 1,
                    next_at = ?2 + MIN(?3, 30 * (1 << MIN(attempts, 6)))
                 WHERE id = ?1",
                rusqlite::params![id.to_hex(), now, MAX_BACKOFF as i64],
            )?;
            Ok(())
        })
    }

    pub fn len(&self) -> opal_core::Result<usize> {
        self.db.with(|c| {
            c.query_row("SELECT COUNT(*) FROM kit_outbox", [], |r| {
                r.get::<_, i64>(0)
            })
            .map(|n| n as usize)
        })
    }

    pub fn is_empty(&self) -> opal_core::Result<bool> {
        self.len().map(|n| n == 0)
    }

    /// Send everything due to `relays`; returns how many went out.
    pub async fn flush(&self, client: &Client, relays: &[RelayUrl]) -> usize {
        let Ok(due) = self.due(50) else {
            return 0;
        };
        let mut sent = 0;
        for ev in due {
            if publish(client, &ev, relays).await.is_ok() {
                let _ = self.sent(&ev.id);
                sent += 1;
            } else {
                let _ = self.failed(&ev.id);
            }
        }
        sent
    }
}

/// `kind:pubkey:d` for replaceable and addressable events, so a newer
/// version replaces a queued older one.
fn coordinate(ev: &Event) -> Option<String> {
    if ev.kind.is_addressable() {
        let d = ev.tags.identifier().unwrap_or_default();
        Some(format!("{}:{}:{}", ev.kind.as_u16(), ev.pubkey.to_hex(), d))
    } else if ev.kind.is_replaceable() {
        Some(format!("{}:{}:", ev.kind.as_u16(), ev.pubkey.to_hex()))
    } else {
        None
    }
}

/// What a relay says about itself (the NIP-11 fields we care about).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RelayInfo {
    pub auth_required: bool,
    pub max_message_length: Option<u64>,
}

#[derive(Deserialize, Default)]
struct Nip11 {
    #[serde(default)]
    limitation: Option<Limitation>,
}

#[derive(Deserialize, Default)]
struct Limitation {
    auth_required: Option<bool>,
    max_message_length: Option<u64>,
}

/// Ask a relay for its NIP-11 document.
pub async fn probe(url: &RelayUrl, timeout: Duration) -> anyhow::Result<RelayInfo> {
    opal_core::identity::ensure_crypto_provider();
    let http = url
        .as_str()
        .replacen("wss://", "https://", 1)
        .replacen("ws://", "http://", 1);
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()?;
    let doc: Nip11 = client
        .get(http)
        .header("Accept", "application/nostr+json")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let lim = doc.limitation.unwrap_or_default();
    Ok(RelayInfo {
        auth_required: lim.auth_required.unwrap_or(false),
        max_message_length: lim.max_message_length,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addressable(keys: &Keys, d: &str, content: &str) -> Event {
        EventBuilder::new(Kind::ApplicationSpecificData, content)
            .tag(Tag::identifier(d))
            .finalize(keys)
            .unwrap()
    }

    #[test]
    fn splits_relay_lists_by_direction() {
        let keys = Keys::generate();
        let ev = EventBuilder::new(Kind::RelayList, "")
            .tag(Tag::parse(["r", "wss://both.example"]).unwrap())
            .tag(Tag::parse(["r", "wss://read.example", "read"]).unwrap())
            .tag(Tag::parse(["r", "wss://write.example", "write"]).unwrap())
            .finalize(&keys)
            .unwrap();
        let list = RelayList::from_event(&ev);
        let s = |v: &[RelayUrl]| v.iter().map(|u| u.to_string()).collect::<Vec<_>>();
        assert_eq!(s(&list.read), ["wss://both.example", "wss://read.example"]);
        assert_eq!(
            s(&list.write),
            ["wss://both.example", "wss://write.example"]
        );
    }

    #[test]
    fn outbox_keeps_only_the_newest_version_of_an_address() {
        let keys = Keys::generate();
        let outbox = Outbox::new(Db::open_in_memory().unwrap()).unwrap();
        outbox.push(&addressable(&keys, "a", "old")).unwrap();
        outbox.push(&addressable(&keys, "a", "new")).unwrap();
        outbox.push(&addressable(&keys, "b", "other")).unwrap();
        let note = EventBuilder::new(Kind::TextNote, "hi")
            .finalize(&keys)
            .unwrap();
        outbox.push(&note).unwrap();
        let due = outbox.due(10).unwrap();
        assert_eq!(due.len(), 3);
        assert!(due.iter().any(|e| e.content == "new"));
        assert!(!due.iter().any(|e| e.content == "old"));

        outbox.failed(&note.id).unwrap();
        assert_eq!(outbox.due(10).unwrap().len(), 2, "backed off");
        outbox.sent(&note.id).unwrap();
        assert_eq!(outbox.len().unwrap(), 2);
    }

    #[tokio::test]
    async fn publishes_and_flushes_to_a_relay() {
        let relay = MockRelay::run().await.unwrap();
        let url = relay.url().await;
        let keys = Keys::generate();
        let client = Client::default();
        client.add_relay(&url).await.unwrap();
        client.connect().and_wait(CONNECT_WAIT).await;

        let outbox = Outbox::new(Db::open_in_memory().unwrap()).unwrap();
        outbox.push(&addressable(&keys, "x", "queued")).unwrap();
        assert_eq!(outbox.flush(&client, std::slice::from_ref(&url)).await, 1);
        assert!(outbox.is_empty().unwrap());

        let got = client
            .fetch_events(Filter::new().author(keys.public_key()))
            .timeout(Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(got.first().map(|e| e.content.as_str()), Some("queued"));

        let direct = addressable(&keys, "y", "direct");
        assert_eq!(publish(&client, &direct, &[url]).await, Ok(1));
    }
}
