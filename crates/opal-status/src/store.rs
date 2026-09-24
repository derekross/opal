//! Scrobble history and the manual status, in the Opal database.

use opal_core::Result;
use opal_core::db::Db;
use rusqlite::params;
use serde::Serialize;

use crate::general::Manual;

const MIGRATIONS: &[&str] = &[r#"
    CREATE TABLE plays (
        id        INTEGER PRIMARY KEY AUTOINCREMENT,
        account   TEXT NOT NULL,
        played_at INTEGER NOT NULL,
        title     TEXT NOT NULL,
        artist    TEXT NOT NULL,
        album     TEXT,
        duration  INTEGER,
        player    TEXT NOT NULL,
        link      TEXT,
        event_id  TEXT
    );
    CREATE INDEX plays_account ON plays(account, played_at DESC);
"#];

const MANUAL_KEY: &str = "status.manual";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Play {
    pub id: i64,
    pub played_at: u64,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration: Option<u64>,
    pub player: String,
    pub link: Option<String>,
    /// Set when the play was published as a kind 1073 event.
    pub event_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Count {
    pub name: String,
    pub plays: u64,
}

#[derive(Clone)]
pub struct StatusStore {
    db: Db,
}

impl StatusStore {
    pub fn new(db: Db) -> Result<Self> {
        db.migrate("status", MIGRATIONS)?;
        Ok(Self { db })
    }

    pub fn add_play(&self, account: &str, p: &Play) -> Result<i64> {
        self.db.with(|c| {
            c.execute(
                "INSERT INTO plays (account, played_at, title, artist, album, duration, player, link, event_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    account,
                    p.played_at as i64,
                    p.title,
                    p.artist,
                    p.album,
                    p.duration.map(|d| d as i64),
                    p.player,
                    p.link,
                    p.event_id
                ],
            )?;
            Ok(c.last_insert_rowid())
        })
    }

    pub fn set_event_id(&self, id: i64, event_id: &str) -> Result<()> {
        self.db.with(|c| {
            c.execute(
                "UPDATE plays SET event_id = ?2 WHERE id = ?1",
                params![id, event_id],
            )
            .map(|_| ())
        })
    }

    pub fn recent(&self, account: &str, limit: u32) -> Result<Vec<Play>> {
        self.db.with(|c| {
            let mut stmt = c.prepare(
                "SELECT id, played_at, title, artist, album, duration, player, link, event_id
                 FROM plays WHERE account = ?1 ORDER BY played_at DESC, id DESC LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![account, limit.clamp(1, 500)], |r| {
                Ok(Play {
                    id: r.get(0)?,
                    played_at: r.get::<_, i64>(1)? as u64,
                    title: r.get(2)?,
                    artist: r.get(3)?,
                    album: r.get(4)?,
                    duration: r.get::<_, Option<i64>>(5)?.map(|d| d as u64),
                    player: r.get(6)?,
                    link: r.get(7)?,
                    event_id: r.get(8)?,
                })
            })?;
            rows.collect()
        })
    }

    /// Most-played artists since `since`.
    pub fn top_artists(&self, account: &str, since: u64, limit: u32) -> Result<Vec<Count>> {
        self.db.with(|c| {
            let mut stmt = c.prepare(
                "SELECT artist, COUNT(*) AS n FROM plays
                 WHERE account = ?1 AND played_at >= ?2 AND artist != ''
                 GROUP BY lower(artist) ORDER BY n DESC, artist LIMIT ?3",
            )?;
            let rows = stmt.query_map(params![account, since as i64, limit], |r| {
                Ok(Count {
                    name: r.get(0)?,
                    plays: r.get::<_, i64>(1)? as u64,
                })
            })?;
            rows.collect()
        })
    }

    pub fn play_count(&self, account: &str, since: u64) -> Result<u64> {
        self.db.with(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM plays WHERE account = ?1 AND played_at >= ?2",
                params![account, since as i64],
                |r| r.get::<_, i64>(0).map(|n| n as u64),
            )
        })
    }

    pub fn clear_plays(&self, account: &str) -> Result<()> {
        self.db.with(|c| {
            c.execute("DELETE FROM plays WHERE account = ?1", [account])
                .map(|_| ())
        })
    }

    pub fn manual(&self) -> Result<Option<Manual>> {
        Ok(self
            .db
            .get_kv(MANUAL_KEY)?
            .and_then(|s| serde_json::from_str(&s).ok()))
    }

    pub fn set_manual(&self, m: Option<&Manual>) -> Result<()> {
        match m {
            Some(m) => self
                .db
                .set_kv(MANUAL_KEY, &serde_json::to_string(m).expect("serializes")),
            None => self.db.with(|c| {
                c.execute("DELETE FROM kv WHERE key = ?1", [MANUAL_KEY])
                    .map(|_| ())
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn play(artist: &str, title: &str, at: u64) -> Play {
        Play {
            id: 0,
            played_at: at,
            title: title.into(),
            artist: artist.into(),
            album: None,
            duration: Some(200),
            player: "spotify".into(),
            link: None,
            event_id: None,
        }
    }

    #[test]
    fn history_and_top_artists() {
        let s = StatusStore::new(Db::open_in_memory().unwrap()).unwrap();
        s.add_play("me", &play("Tool", "Lateralus", 10)).unwrap();
        s.add_play("me", &play("tool", "Schism", 20)).unwrap();
        let id = s.add_play("me", &play("Björk", "Joga", 30)).unwrap();
        s.add_play("other", &play("Tool", "Parabola", 40)).unwrap();
        s.set_event_id(id, "abc").unwrap();

        let recent = s.recent("me", 10).unwrap();
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].title, "Joga");
        assert_eq!(recent[0].event_id.as_deref(), Some("abc"));
        let top = s.top_artists("me", 0, 5).unwrap();
        assert_eq!(
            (top[0].name.to_lowercase().as_str(), top[0].plays),
            ("tool", 2)
        );
        assert_eq!(s.play_count("me", 15).unwrap(), 2);
    }

    #[test]
    fn manual_status_roundtrip() {
        let s = StatusStore::new(Db::open_in_memory().unwrap()).unwrap();
        assert_eq!(s.manual().unwrap(), None);
        let m = Manual {
            text: "At a conference".into(),
            link: None,
            expires_at: Some(5),
        };
        s.set_manual(Some(&m)).unwrap();
        assert_eq!(s.manual().unwrap(), Some(m));
        s.set_manual(None).unwrap();
        assert_eq!(s.manual().unwrap(), None);
    }
}
