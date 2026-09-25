//! Notification tables: the feed, read markers, the notes they refer to, a
//! small profile cache, and gift wraps waiting for an unlock.

use opal_core::Result;
use opal_core::accounts::Profile;
use opal_core::db::Db;
use rusqlite::{OptionalExtension, params};
use serde::Serialize;

use crate::classify::{NotifType, Notification};

const MIGRATIONS: &[&str] = &[r#"
    CREATE TABLE notifications (
        id          TEXT NOT NULL,
        account     TEXT NOT NULL,
        type        TEXT NOT NULL,
        kind        INTEGER NOT NULL,
        author      TEXT NOT NULL,
        created_at  INTEGER NOT NULL,
        detail      TEXT NOT NULL,
        ref_id      TEXT,
        sats        INTEGER,
        media       TEXT,
        relay       TEXT,
        received_at INTEGER NOT NULL,
        PRIMARY KEY (account, id)
    );
    CREATE INDEX notifications_feed ON notifications(account, created_at DESC);
    CREATE TABLE notify_state (
        account   TEXT PRIMARY KEY,
        last_read INTEGER NOT NULL DEFAULT 0,
        last_seen INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE ref_events (
        id      TEXT PRIMARY KEY,
        author  TEXT NOT NULL,
        kind    INTEGER NOT NULL,
        content TEXT NOT NULL
    );
    CREATE TABLE profile_cache (
        pubkey       TEXT PRIMARY KEY,
        name         TEXT,
        display_name TEXT,
        picture      TEXT,
        nip05        TEXT,
        fetched_at   INTEGER NOT NULL
    );
    CREATE TABLE pending_wraps (
        id          TEXT NOT NULL,
        account     TEXT NOT NULL,
        event_json  TEXT NOT NULL,
        received_at INTEGER NOT NULL,
        PRIMARY KEY (account, id)
    );
"#];

/// Keep this many notifications per account.
const KEEP: i64 = 500;
/// Gift wraps waiting for an unlock: at most this many, this big.
const MAX_PENDING_WRAPS: i64 = 1000;
const MAX_WRAP_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct StoredNotification {
    #[serde(flatten)]
    pub n: Notification,
    pub unread: bool,
    pub author_name: Option<String>,
    pub author_picture: Option<String>,
    /// Text of the note this refers to (your note that was liked, …).
    pub context: Option<String>,
    pub ref_author: Option<String>,
    pub ref_kind: Option<u16>,
}

#[derive(Clone)]
pub struct NotifyStore {
    db: Db,
}

impl NotifyStore {
    pub fn new(db: Db) -> Result<Self> {
        db.migrate("notify", MIGRATIONS)?;
        Ok(Self { db })
    }

    /// Returns `true` if it wasn't stored before.
    pub fn insert(&self, account: &str, n: &Notification, now: u64) -> Result<bool> {
        let inserted = self.db.with(|c| {
            c.execute(
                "INSERT OR IGNORE INTO notifications
                   (id, account, type, kind, author, created_at, detail, ref_id, sats, media, relay, received_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    n.id,
                    account,
                    n.ntype.as_str(),
                    n.kind,
                    n.author,
                    n.created_at as i64,
                    n.detail,
                    n.ref_id,
                    n.sats.map(|s| s as i64),
                    n.media,
                    n.relay,
                    now as i64
                ],
            )
        })? == 1;
        if inserted {
            self.db.with(|c| {
                c.execute(
                    "DELETE FROM notifications WHERE account = ?1 AND id NOT IN (
                       SELECT id FROM notifications WHERE account = ?1
                       ORDER BY created_at DESC LIMIT ?2)",
                    params![account, KEEP],
                )
            })?;
        }
        Ok(inserted)
    }

    pub fn list(
        &self,
        account: &str,
        limit: u32,
        before: Option<u64>,
    ) -> Result<Vec<StoredNotification>> {
        let last_read = self.last_read(account)?;
        let limit = if limit == 0 { 100 } else { limit.min(500) };
        self.db.with(|c| {
            let mut stmt = c.prepare(
                "SELECT n.id, n.type, n.kind, n.author, n.created_at, n.detail, n.ref_id, n.sats,
                        n.media, n.relay, n.received_at,
                        COALESCE(p.display_name, p.name), p.picture,
                        r.content, r.author, r.kind
                 FROM notifications n
                 LEFT JOIN profile_cache p ON p.pubkey = n.author
                 LEFT JOIN ref_events r ON r.id = n.ref_id
                 WHERE n.account = ?1 AND (?2 IS NULL OR n.created_at < ?2)
                 ORDER BY n.created_at DESC LIMIT ?3",
            )?;
            let rows = stmt.query_map(params![account, before.map(|b| b as i64), limit], |r| {
                let ntype: String = r.get(1)?;
                let created_at = r.get::<_, i64>(4)? as u64;
                let received_at = r.get::<_, i64>(10)? as u64;
                Ok(StoredNotification {
                    n: Notification {
                        id: r.get(0)?,
                        ntype: NotifType::parse(&ntype).unwrap_or(NotifType::Mention),
                        kind: r.get(2)?,
                        author: r.get(3)?,
                        created_at,
                        detail: r.get(5)?,
                        ref_id: r.get(6)?,
                        sats: r.get::<_, Option<i64>>(7)?.map(|s| s as u64),
                        media: r.get(8)?,
                        relay: r.get(9)?,
                    },
                    // DMs use a randomized timestamp; unread goes by arrival.
                    unread: if ntype == "dm" {
                        received_at > last_read
                    } else {
                        created_at > last_read
                    },
                    author_name: r.get(11)?,
                    author_picture: r.get(12)?,
                    context: r.get(13)?,
                    ref_author: r.get(14)?,
                    ref_kind: r.get(15)?,
                })
            })?;
            rows.collect()
        })
    }

    pub fn get(&self, account: &str, id: &str) -> Result<Option<StoredNotification>> {
        // Small table; reuse the list query's joins.
        Ok(self
            .list(account, 500, None)?
            .into_iter()
            .find(|n| n.n.id == id))
    }

    pub fn unread_count(&self, account: &str) -> Result<u64> {
        let last_read = self.last_read(account)? as i64;
        self.db.with(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM notifications WHERE account = ?1 AND
                   ((type = 'dm' AND received_at > ?2) OR (type != 'dm' AND created_at > ?2))",
                params![account, last_read],
                |r| r.get::<_, i64>(0).map(|n| n as u64),
            )
        })
    }

    pub fn last_read(&self, account: &str) -> Result<u64> {
        self.state(account).map(|s| s.0)
    }

    pub fn last_seen(&self, account: &str) -> Result<u64> {
        self.state(account).map(|s| s.1)
    }

    fn state(&self, account: &str) -> Result<(u64, u64)> {
        self.db.with(|c| {
            c.query_row(
                "SELECT last_read, last_seen FROM notify_state WHERE account = ?1",
                [account],
                |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64)),
            )
            .optional()
            .map(|o| o.unwrap_or((0, 0)))
        })
    }

    pub fn mark_read(&self, account: &str, at: u64) -> Result<()> {
        self.db.with(|c| {
            c.execute(
                "INSERT INTO notify_state (account, last_read) VALUES (?1, ?2)
                 ON CONFLICT(account) DO UPDATE SET last_read = MAX(last_read, excluded.last_read)",
                params![account, at as i64],
            )
            .map(|_| ())
        })
    }

    pub fn set_last_seen(&self, account: &str, at: u64) -> Result<()> {
        self.db.with(|c| {
            c.execute(
                "INSERT INTO notify_state (account, last_seen) VALUES (?1, ?2)
                 ON CONFLICT(account) DO UPDATE SET last_seen = MAX(last_seen, excluded.last_seen)",
                params![account, at as i64],
            )
            .map(|_| ())
        })
    }

    pub fn clear(&self, account: &str) -> Result<()> {
        self.db.with(|c| {
            c.execute("DELETE FROM notifications WHERE account = ?1", [account])
                .map(|_| ())
        })
    }

    /// Remove notifications from authors that are now muted.
    pub fn remove_authors(&self, account: &str, authors: &[String]) -> Result<usize> {
        let mut removed = 0;
        for a in authors {
            removed += self.db.with(|c| {
                c.execute(
                    "DELETE FROM notifications WHERE account = ?1 AND author = ?2",
                    params![account, a],
                )
            })?;
        }
        Ok(removed)
    }

    pub fn put_ref_event(&self, id: &str, author: &str, kind: u16, content: &str) -> Result<()> {
        let content: String = content.chars().take(500).collect();
        self.db.with(|c| {
            c.execute(
                "INSERT OR REPLACE INTO ref_events (id, author, kind, content) VALUES (?1, ?2, ?3, ?4)",
                params![id, author, kind, content],
            )
            .map(|_| ())
        })
    }

    pub fn has_ref_event(&self, id: &str) -> Result<bool> {
        self.db.with(|c| {
            c.query_row("SELECT 1 FROM ref_events WHERE id = ?1", [id], |_| Ok(()))
                .optional()
                .map(|o| o.is_some())
        })
    }

    pub fn put_profile(&self, pubkey: &str, p: &Profile, at: u64) -> Result<()> {
        self.db.with(|c| {
            c.execute(
                "INSERT OR REPLACE INTO profile_cache (pubkey, name, display_name, picture, nip05, fetched_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![pubkey, p.name, p.display_name, p.picture, p.nip05, at as i64],
            )
            .map(|_| ())
        })
    }

    pub fn profile(&self, pubkey: &str) -> Result<Option<(Profile, u64)>> {
        self.db.with(|c| {
            c.query_row(
                "SELECT name, display_name, picture, nip05, fetched_at FROM profile_cache WHERE pubkey = ?1",
                [pubkey],
                |r| {
                    Ok((
                        Profile {
                            name: r.get(0)?,
                            display_name: r.get(1)?,
                            picture: r.get(2)?,
                            nip05: r.get(3)?,
                            about: None,
                        },
                        r.get::<_, i64>(4)? as u64,
                    ))
                },
            )
            .optional()
        })
    }

    /// Keep a gift wrap for when the key is available. Anyone can send
    /// these, so size and count are capped (oldest dropped).
    pub fn add_pending_wrap(&self, account: &str, id: &str, json: &str, now: u64) -> Result<()> {
        if json.len() > MAX_WRAP_BYTES {
            return Ok(());
        }
        self.db.with(|c| {
            c.execute(
                "INSERT OR IGNORE INTO pending_wraps (id, account, event_json, received_at) VALUES (?1, ?2, ?3, ?4)",
                params![id, account, json, now as i64],
            )?;
            c.execute(
                "DELETE FROM pending_wraps WHERE account = ?1 AND id NOT IN (
                   SELECT id FROM pending_wraps WHERE account = ?1
                   ORDER BY received_at DESC LIMIT ?2)",
                params![account, MAX_PENDING_WRAPS],
            )
            .map(|_| ())
        })
    }

    /// Drop cached profiles and referenced notes no notification needs.
    pub fn prune_orphans(&self, me: &str) -> Result<()> {
        self.db.with(|c| {
            c.execute(
                "DELETE FROM ref_events WHERE id NOT IN (SELECT ref_id FROM notifications WHERE ref_id IS NOT NULL)",
                [],
            )?;
            c.execute(
                "DELETE FROM profile_cache WHERE pubkey != ?1 AND pubkey NOT IN (SELECT author FROM notifications)",
                [me],
            )
            .map(|_| ())
        })
    }

    /// Take (and delete) the gift wraps that arrived while locked.
    pub fn take_pending_wraps(&self, account: &str) -> Result<Vec<(String, u64)>> {
        self.db.with(|c| {
            let rows: Vec<(String, u64)> = {
                let mut stmt = c.prepare(
                    "SELECT event_json, received_at FROM pending_wraps WHERE account = ?1 ORDER BY received_at",
                )?;
                stmt.query_map([account], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u64)))?
                    .collect::<rusqlite::Result<_>>()?
            };
            c.execute("DELETE FROM pending_wraps WHERE account = ?1", [account])?;
            Ok(rows)
        })
    }

    /// Record when a DM arrived (unread for DMs goes by arrival time).
    pub fn set_received_at(&self, account: &str, id: &str, at: u64) -> Result<()> {
        self.db.with(|c| {
            c.execute(
                "UPDATE notifications SET received_at = ?3 WHERE account = ?1 AND id = ?2",
                params![account, id, at as i64],
            )
            .map(|_| ())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(id: &str, t: NotifType, created: u64) -> Notification {
        Notification {
            id: id.into(),
            ntype: t,
            kind: 1,
            author: "a".repeat(64),
            created_at: created,
            detail: "hi".into(),
            ref_id: Some("r".repeat(64)),
            sats: None,
            media: None,
            relay: None,
        }
    }

    #[test]
    fn feed_unread_and_context() {
        let s = NotifyStore::new(Db::open_in_memory().unwrap()).unwrap();
        assert!(s.insert("me", &n("1", NotifType::Reply, 100), 100).unwrap());
        assert!(
            !s.insert("me", &n("1", NotifType::Reply, 100), 100).unwrap(),
            "dedupe"
        );
        s.insert("me", &n("2", NotifType::Reaction, 200), 200)
            .unwrap();
        s.insert("other", &n("3", NotifType::Reply, 300), 300)
            .unwrap();
        assert_eq!(s.unread_count("me").unwrap(), 2);

        s.put_ref_event(&"r".repeat(64), &"b".repeat(64), 1, "my original note")
            .unwrap();
        s.put_profile(
            &"a".repeat(64),
            &Profile {
                name: Some("alice".into()),
                ..Default::default()
            },
            1,
        )
        .unwrap();
        let list = s.list("me", 10, None).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].n.id, "2", "newest first");
        assert_eq!(list[0].context.as_deref(), Some("my original note"));
        assert_eq!(list[0].author_name.as_deref(), Some("alice"));

        s.mark_read("me", 150).unwrap();
        assert_eq!(s.unread_count("me").unwrap(), 1);
        s.mark_read("me", 50).unwrap();
        assert_eq!(
            s.unread_count("me").unwrap(),
            1,
            "read marker never goes back"
        );
        assert_eq!(s.list("me", 10, Some(200)).unwrap().len(), 1, "paging");
    }

    #[test]
    fn dms_are_unread_by_arrival() {
        let s = NotifyStore::new(Db::open_in_memory().unwrap()).unwrap();
        // Gift wrap timestamps are randomized into the past.
        s.insert("me", &n("dm", NotifType::Dm, 10), 500).unwrap();
        s.mark_read("me", 400).unwrap();
        assert_eq!(s.unread_count("me").unwrap(), 1);
    }

    #[test]
    fn pending_wraps_are_taken_once() {
        let s = NotifyStore::new(Db::open_in_memory().unwrap()).unwrap();
        s.add_pending_wrap("me", "w1", "{}", 1).unwrap();
        s.add_pending_wrap("me", "w1", "{}", 1).unwrap();
        assert_eq!(s.take_pending_wraps("me").unwrap().len(), 1);
        assert!(s.take_pending_wraps("me").unwrap().is_empty());
    }
}
