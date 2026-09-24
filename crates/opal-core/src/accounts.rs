//! Account details that are not secret: nickname, cached profile, which one
//! is selected. The keys themselves live in the [`Vault`](crate::vault::Vault).

use nostr::key::PublicKey;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::Result;
use crate::db::Db;

const MIGRATIONS: &[&str] = &[r#"
    CREATE TABLE accounts (
        pubkey       TEXT PRIMARY KEY,
        nickname     TEXT,
        name         TEXT,
        display_name TEXT,
        picture      TEXT,
        nip05        TEXT,
        about        TEXT,
        fetched_at   INTEGER,
        added_at     INTEGER NOT NULL
    );
"#];

const CURRENT_KEY: &str = "current_account";

/// Profile fields we show (from kind 0).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Profile {
    pub name: Option<String>,
    pub display_name: Option<String>,
    pub picture: Option<String>,
    pub nip05: Option<String>,
    pub about: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AccountMeta {
    pub pubkey: String,
    pub nickname: Option<String>,
    #[serde(flatten)]
    pub profile: Profile,
    pub fetched_at: Option<u64>,
    pub added_at: u64,
}

impl AccountMeta {
    /// Nickname, then profile names, then a short npub.
    pub fn label(&self) -> String {
        self.nickname
            .clone()
            .or_else(|| self.profile.display_name.clone())
            .or_else(|| self.profile.name.clone())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| match PublicKey::from_hex(&self.pubkey) {
                Ok(pk) => crate::vault::short_npub(&pk),
                Err(_) => self.pubkey.clone(),
            })
    }
}

#[derive(Clone)]
pub struct Accounts {
    db: Db,
}

impl Accounts {
    pub fn new(db: Db) -> Result<Self> {
        db.migrate("accounts", MIGRATIONS)?;
        Ok(Self { db })
    }

    pub fn add(&self, pk: &PublicKey, nickname: Option<&str>, now: u64) -> Result<()> {
        self.db.with(|c| {
            c.execute(
                "INSERT INTO accounts (pubkey, nickname, added_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(pubkey) DO UPDATE SET nickname = COALESCE(excluded.nickname, nickname)",
                params![pk.to_hex(), nickname.filter(|n| !n.trim().is_empty()), now as i64],
            )
            .map(|_| ())
        })?;
        if self.current()?.is_none() {
            self.set_current(pk)?;
        }
        Ok(())
    }

    pub fn remove(&self, pk: &PublicKey) -> Result<()> {
        self.db.with(|c| {
            c.execute("DELETE FROM accounts WHERE pubkey = ?1", [pk.to_hex()])
                .map(|_| ())
        })?;
        if self.current()? == Some(*pk) {
            match self.list()?.first() {
                Some(next) => self.db.set_kv(CURRENT_KEY, &next.pubkey)?,
                None => self.db.with(|c| {
                    c.execute("DELETE FROM kv WHERE key = ?1", [CURRENT_KEY])
                        .map(|_| ())
                })?,
            }
        }
        Ok(())
    }

    pub fn set_nickname(&self, pk: &PublicKey, nickname: Option<&str>) -> Result<()> {
        self.db.with(|c| {
            c.execute(
                "UPDATE accounts SET nickname = ?2 WHERE pubkey = ?1",
                params![pk.to_hex(), nickname.filter(|n| !n.trim().is_empty())],
            )
            .map(|_| ())
        })
    }

    pub fn set_profile(&self, pk: &PublicKey, p: &Profile, at: u64) -> Result<()> {
        self.db.with(|c| {
            c.execute(
                "UPDATE accounts SET name = ?2, display_name = ?3, picture = ?4, nip05 = ?5,
                                     about = ?6, fetched_at = ?7
                 WHERE pubkey = ?1",
                params![
                    pk.to_hex(),
                    p.name,
                    p.display_name,
                    p.picture,
                    p.nip05,
                    p.about,
                    at as i64
                ],
            )
            .map(|_| ())
        })
    }

    pub fn get(&self, pk: &PublicKey) -> Result<Option<AccountMeta>> {
        self.db.with(|c| {
            c.query_row(
                "SELECT pubkey, nickname, name, display_name, picture, nip05, about, fetched_at, added_at
                 FROM accounts WHERE pubkey = ?1",
                [pk.to_hex()],
                row,
            )
            .optional()
        })
    }

    pub fn list(&self) -> Result<Vec<AccountMeta>> {
        self.db.with(|c| {
            let mut stmt = c.prepare(
                "SELECT pubkey, nickname, name, display_name, picture, nip05, about, fetched_at, added_at
                 FROM accounts ORDER BY added_at, pubkey",
            )?;
            let rows = stmt.query_map([], row)?;
            rows.collect()
        })
    }

    pub fn current(&self) -> Result<Option<PublicKey>> {
        Ok(self
            .db
            .get_kv(CURRENT_KEY)?
            .and_then(|s| PublicKey::from_hex(&s).ok()))
    }

    pub fn set_current(&self, pk: &PublicKey) -> Result<()> {
        self.db.set_kv(CURRENT_KEY, &pk.to_hex())
    }
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<AccountMeta> {
    Ok(AccountMeta {
        pubkey: r.get(0)?,
        nickname: r.get(1)?,
        profile: Profile {
            name: r.get(2)?,
            display_name: r.get(3)?,
            picture: r.get(4)?,
            nip05: r.get(5)?,
            about: r.get(6)?,
        },
        fetched_at: r.get::<_, Option<i64>>(7)?.map(|t| t as u64),
        added_at: r.get::<_, i64>(8)? as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::key::Keys;

    #[test]
    fn current_follows_adds_and_removes() {
        let a = Accounts::new(Db::open_in_memory().unwrap()).unwrap();
        let (k1, k2) = (Keys::generate().public_key(), Keys::generate().public_key());
        a.add(&k1, Some("main"), 1).unwrap();
        a.add(&k2, None, 2).unwrap();
        assert_eq!(
            a.current().unwrap(),
            Some(k1),
            "first account becomes current"
        );
        a.set_current(&k2).unwrap();
        a.remove(&k2).unwrap();
        assert_eq!(a.current().unwrap(), Some(k1));
        a.remove(&k1).unwrap();
        assert_eq!(a.current().unwrap(), None);
    }

    #[test]
    fn labels_prefer_nickname_then_profile() {
        let a = Accounts::new(Db::open_in_memory().unwrap()).unwrap();
        let pk = Keys::generate().public_key();
        a.add(&pk, None, 1).unwrap();
        assert!(a.get(&pk).unwrap().unwrap().label().starts_with("npub1"));
        a.set_profile(
            &pk,
            &Profile {
                name: Some("derek".into()),
                ..Default::default()
            },
            5,
        )
        .unwrap();
        assert_eq!(a.get(&pk).unwrap().unwrap().label(), "derek");
        a.set_nickname(&pk, Some("Work")).unwrap();
        assert_eq!(a.get(&pk).unwrap().unwrap().label(), "Work");
    }
}
