//! The signer's tables: apps, permission rules, the activity log, and ids of
//! requests already handled.
//!
//! Key material is not stored here: per-app transport keys live in the
//! keyring and bunker secrets are kept only as SHA-256 hashes.

use nostr_sdk::prelude::{PublicKey, RelayUrl, Timestamp};
use opal_core::Result;
use opal_core::config::Policy;
use opal_core::db::Db;
use rusqlite::{OptionalExtension, Row, params};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::permissions::{Rule, Source};
use crate::perms::PermSpec;
use crate::protocol::Method;

const MIGRATIONS: &[&str] = &[r#"
    CREATE TABLE apps (
        id                TEXT PRIMARY KEY,
        account           TEXT NOT NULL,
        transport_pubkey  TEXT NOT NULL UNIQUE,
        client_pubkey     TEXT,
        secret_hash       TEXT,
        name              TEXT,
        url               TEXT,
        image             TEXT,
        relays            TEXT NOT NULL,
        policy            TEXT NOT NULL,
        requested_perms   TEXT NOT NULL,
        created_at        INTEGER NOT NULL,
        last_used         INTEGER,
        expires_unused_at INTEGER
    );
    CREATE TABLE rules (
        app_id     TEXT NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
        method     TEXT NOT NULL,
        kind       INTEGER NOT NULL,   -- -1 = any kind
        allow      INTEGER NOT NULL,
        until      INTEGER,            -- NULL = forever
        created_at INTEGER NOT NULL,
        PRIMARY KEY (app_id, method, kind)
    );
    CREATE TABLE activity (
        id       INTEGER PRIMARY KEY AUTOINCREMENT,
        at       INTEGER NOT NULL,
        app_id   TEXT NOT NULL,
        app_name TEXT NOT NULL,
        account  TEXT NOT NULL,
        method   TEXT NOT NULL,
        kind     INTEGER,
        allowed  INTEGER NOT NULL,
        source   TEXT NOT NULL,
        reason   TEXT
    );
    CREATE INDEX activity_at ON activity(at);
    CREATE INDEX activity_app ON activity(app_id, at);
    CREATE TABLE seen_requests (
        id TEXT PRIMARY KEY,
        at INTEGER NOT NULL
    );
"#];

/// An app as stored, without its transport secret key.
#[derive(Debug, Clone)]
pub struct AppRecord {
    pub id: String,
    pub account: PublicKey,
    pub transport_pubkey: PublicKey,
    pub client: Option<PublicKey>,
    pub secret_hash: Option<String>,
    pub name: Option<String>,
    pub url: Option<String>,
    pub image: Option<String>,
    pub relays: Vec<RelayUrl>,
    pub policy: Policy,
    pub requested_perms: Vec<PermSpec>,
    pub created_at: Timestamp,
    pub last_used: Option<Timestamp>,
    pub expires_unused_at: Option<Timestamp>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActivityEntry {
    pub id: i64,
    pub at: u64,
    pub app_id: String,
    pub app_name: String,
    pub account: String,
    pub method: Method,
    pub kind: Option<u16>,
    pub kind_label: Option<String>,
    pub allowed: bool,
    pub source: Source,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ActivityQuery {
    pub app_id: Option<String>,
    pub allowed: Option<bool>,
    /// Only entries with an id lower than this (for paging).
    pub before_id: Option<i64>,
    pub limit: u32,
}

#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
pub struct ActivityStats {
    pub allowed: u64,
    pub denied: u64,
}

pub fn hash_secret(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))
}

#[derive(Clone)]
pub struct SignerStore {
    db: Db,
}

impl SignerStore {
    pub fn new(db: Db) -> Result<Self> {
        db.migrate("signer", MIGRATIONS)?;
        Ok(Self { db })
    }

    pub fn save_app(&self, app: &AppRecord) -> Result<()> {
        let relays = serde_json::to_string(&app.relays).expect("relays serialize");
        let perms = serde_json::to_string(&app.requested_perms).expect("perms serialize");
        let policy = policy_str(app.policy);
        self.db.with(|c| {
            c.execute(
                "INSERT INTO apps (id, account, transport_pubkey, client_pubkey, secret_hash, name,
                                   url, image, relays, policy, requested_perms, created_at,
                                   last_used, expires_unused_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                 ON CONFLICT(id) DO UPDATE SET
                    client_pubkey = excluded.client_pubkey,
                    secret_hash = excluded.secret_hash,
                    name = excluded.name,
                    url = excluded.url,
                    image = excluded.image,
                    relays = excluded.relays,
                    policy = excluded.policy,
                    requested_perms = excluded.requested_perms,
                    last_used = excluded.last_used,
                    expires_unused_at = excluded.expires_unused_at",
                params![
                    app.id,
                    app.account.to_hex(),
                    app.transport_pubkey.to_hex(),
                    app.client.map(|c| c.to_hex()),
                    app.secret_hash,
                    app.name,
                    app.url,
                    app.image,
                    relays,
                    policy,
                    perms,
                    app.created_at.as_secs() as i64,
                    app.last_used.map(|t| t.as_secs() as i64),
                    app.expires_unused_at.map(|t| t.as_secs() as i64),
                ],
            )
            .map(|_| ())
        })
    }

    pub fn touch_app(&self, id: &str, at: Timestamp) -> Result<()> {
        self.db.with(|c| {
            c.execute(
                "UPDATE apps SET last_used = ?2 WHERE id = ?1",
                params![id, at.as_secs() as i64],
            )
            .map(|_| ())
        })
    }

    pub fn load_apps(&self) -> Result<Vec<AppRecord>> {
        self.db.with(|c| {
            let mut stmt = c.prepare("SELECT * FROM apps ORDER BY created_at DESC")?;
            let rows = stmt.query_map([], app_from_row)?;
            let mut out = Vec::new();
            for r in rows {
                // Skip rows we can't parse rather than failing startup.
                if let Some(app) = r? {
                    out.push(app);
                }
            }
            Ok(out)
        })
    }

    pub fn delete_app(&self, id: &str) -> Result<()> {
        self.db.with(|c| {
            c.execute("DELETE FROM apps WHERE id = ?1", [id])
                .map(|_| ())
        })
    }

    pub fn rules(&self, app_id: &str) -> Result<Vec<Rule>> {
        self.db.with(|c| {
            let mut stmt = c.prepare(
                "SELECT app_id, method, kind, allow, until, created_at FROM rules
                 WHERE app_id = ?1 ORDER BY method, kind",
            )?;
            let rows = stmt.query_map([app_id], |r| {
                let kind: i64 = r.get(2)?;
                Ok(Rule {
                    app_id: r.get(0)?,
                    method: Method::from(r.get::<_, String>(1)?),
                    kind: u16::try_from(kind).ok(),
                    allow: r.get(3)?,
                    until: r.get::<_, Option<i64>>(4)?.map(|t| t as u64),
                    created_at: r.get::<_, i64>(5)? as u64,
                })
            })?;
            rows.collect()
        })
    }

    /// Insert or replace the rule for `(app, method, kind)`.
    pub fn put_rule(&self, rule: &Rule) -> Result<()> {
        self.db.with(|c| {
            c.execute(
                "INSERT INTO rules (app_id, method, kind, allow, until, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(app_id, method, kind) DO UPDATE SET
                    allow = excluded.allow, until = excluded.until, created_at = excluded.created_at",
                params![
                    rule.app_id,
                    rule.method.as_str(),
                    rule.kind.map_or(-1, i64::from),
                    rule.allow,
                    rule.until.map(|t| t as i64),
                    rule.created_at as i64,
                ],
            )
            .map(|_| ())
        })
    }

    pub fn delete_rule(&self, app_id: &str, method: &Method, kind: Option<u16>) -> Result<()> {
        self.db.with(|c| {
            c.execute(
                "DELETE FROM rules WHERE app_id = ?1 AND method = ?2 AND kind = ?3",
                params![app_id, method.as_str(), kind.map_or(-1, i64::from)],
            )
            .map(|_| ())
        })
    }

    pub fn clear_rules(&self, app_id: &str) -> Result<()> {
        self.db.with(|c| {
            c.execute("DELETE FROM rules WHERE app_id = ?1", [app_id])
                .map(|_| ())
        })
    }

    pub fn log_activity(&self, e: &ActivityEntry) -> Result<()> {
        self.db.with(|c| {
            c.execute(
                "INSERT INTO activity (at, app_id, app_name, account, method, kind, allowed, source, reason)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    e.at as i64,
                    e.app_id,
                    e.app_name,
                    e.account,
                    e.method.as_str(),
                    e.kind.map(i64::from),
                    e.allowed,
                    serde_json::to_value(e.source).ok().and_then(|v| v.as_str().map(String::from)),
                    e.reason,
                ],
            )
            .map(|_| ())
        })
    }

    pub fn activity(&self, q: &ActivityQuery) -> Result<Vec<ActivityEntry>> {
        let limit = if q.limit == 0 { 100 } else { q.limit.min(1000) };
        self.db.with(|c| {
            let mut stmt = c.prepare(
                "SELECT id, at, app_id, app_name, account, method, kind, allowed, source, reason
                 FROM activity
                 WHERE (?1 IS NULL OR app_id = ?1)
                   AND (?2 IS NULL OR allowed = ?2)
                   AND (?3 IS NULL OR id < ?3)
                 ORDER BY id DESC LIMIT ?4",
            )?;
            let rows = stmt.query_map(params![q.app_id, q.allowed, q.before_id, limit], |r| {
                let kind: Option<u16> = r
                    .get::<_, Option<i64>>(6)?
                    .and_then(|k| u16::try_from(k).ok());
                let source: String = r.get(8)?;
                Ok(ActivityEntry {
                    id: r.get(0)?,
                    at: r.get::<_, i64>(1)? as u64,
                    app_id: r.get(2)?,
                    app_name: r.get(3)?,
                    account: r.get(4)?,
                    method: Method::from(r.get::<_, String>(5)?),
                    kind,
                    kind_label: kind.map(crate::kinds::label),
                    allowed: r.get(7)?,
                    source: serde_json::from_value(serde_json::Value::String(source))
                        .unwrap_or(Source::Error),
                    reason: r.get(9)?,
                })
            })?;
            rows.collect()
        })
    }

    pub fn activity_stats(&self, since: u64) -> Result<ActivityStats> {
        self.db.with(|c| {
            c.query_row(
                "SELECT COALESCE(SUM(allowed), 0), COALESCE(SUM(1 - allowed), 0)
                 FROM activity WHERE at >= ?1",
                [since as i64],
                |r| {
                    Ok(ActivityStats {
                        allowed: r.get::<_, i64>(0)? as u64,
                        denied: r.get::<_, i64>(1)? as u64,
                    })
                },
            )
        })
    }

    pub fn prune_activity(&self, before: u64) -> Result<usize> {
        self.db
            .with(|c| c.execute("DELETE FROM activity WHERE at < ?1", [before as i64]))
    }

    pub fn clear_activity(&self) -> Result<()> {
        self.db
            .with(|c| c.execute("DELETE FROM activity", []).map(|_| ()))
    }

    /// Record a request id; `false` if it was already handled.
    pub fn mark_seen(&self, id: &str, at: u64) -> Result<bool> {
        self.db.with(|c| {
            c.execute(
                "INSERT OR IGNORE INTO seen_requests (id, at) VALUES (?1, ?2)",
                params![id, at as i64],
            )
            .map(|n| n == 1)
        })
    }

    pub fn prune_seen(&self, before: u64) -> Result<usize> {
        self.db
            .with(|c| c.execute("DELETE FROM seen_requests WHERE at < ?1", [before as i64]))
    }

    pub fn app_exists(&self, id: &str) -> Result<bool> {
        self.db.with(|c| {
            c.query_row("SELECT 1 FROM apps WHERE id = ?1", [id], |_| Ok(()))
                .optional()
                .map(|o| o.is_some())
        })
    }
}

pub fn policy_str(p: Policy) -> &'static str {
    match p {
        Policy::Basic => "basic",
        Policy::Manual => "manual",
        Policy::FullTrust => "full-trust",
    }
}

fn parse_policy(s: &str) -> Policy {
    match s {
        "manual" => Policy::Manual,
        "full-trust" => Policy::FullTrust,
        _ => Policy::Basic,
    }
}

fn app_from_row(r: &Row<'_>) -> rusqlite::Result<Option<AppRecord>> {
    let pk = |s: Option<String>| s.and_then(|s| PublicKey::from_hex(&s).ok());
    let ts = |t: Option<i64>| t.map(|t| Timestamp::from(t as u64));
    let (Some(account), Some(transport)) = (pk(r.get("account")?), pk(r.get("transport_pubkey")?))
    else {
        return Ok(None);
    };
    let relays: String = r.get("relays")?;
    let perms: String = r.get("requested_perms")?;
    Ok(Some(AppRecord {
        id: r.get("id")?,
        account,
        transport_pubkey: transport,
        client: pk(r.get("client_pubkey")?),
        secret_hash: r.get("secret_hash")?,
        name: r.get("name")?,
        url: r.get("url")?,
        image: r.get("image")?,
        relays: serde_json::from_str(&relays).unwrap_or_default(),
        policy: parse_policy(&r.get::<_, String>("policy")?),
        requested_perms: serde_json::from_str(&perms).unwrap_or_default(),
        created_at: Timestamp::from(r.get::<_, i64>("created_at")? as u64),
        last_used: ts(r.get("last_used")?),
        expires_unused_at: ts(r.get("expires_unused_at")?),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr_sdk::prelude::Keys;

    fn store() -> SignerStore {
        SignerStore::new(Db::open_in_memory().unwrap()).unwrap()
    }

    fn app(id: &str) -> AppRecord {
        AppRecord {
            id: id.into(),
            account: Keys::generate().public_key(),
            transport_pubkey: Keys::generate().public_key(),
            client: None,
            secret_hash: Some(hash_secret("s")),
            name: Some("App".into()),
            url: None,
            image: None,
            relays: vec![RelayUrl::parse("wss://relay.example.com").unwrap()],
            policy: Policy::Manual,
            requested_perms: crate::perms::parse_perms("sign_event:1"),
            created_at: Timestamp::from(100),
            last_used: None,
            expires_unused_at: None,
        }
    }

    #[test]
    fn apps_roundtrip_and_cascade() {
        let s = store();
        let mut a = app("a");
        s.save_app(&a).unwrap();
        a.client = Some(Keys::generate().public_key());
        a.secret_hash = None;
        s.save_app(&a).unwrap();

        let loaded = s.load_apps().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].client, a.client);
        assert_eq!(loaded[0].secret_hash, None);
        assert_eq!(loaded[0].policy, Policy::Manual);
        assert_eq!(loaded[0].relays, a.relays);
        assert_eq!(loaded[0].requested_perms, a.requested_perms);

        s.put_rule(&Rule {
            app_id: "a".into(),
            method: Method::SignEvent,
            kind: Some(1),
            allow: true,
            until: None,
            created_at: 1,
        })
        .unwrap();
        assert_eq!(s.rules("a").unwrap().len(), 1);
        s.delete_app("a").unwrap();
        assert!(s.rules("a").unwrap().is_empty(), "rules go with the app");
    }

    #[test]
    fn rules_upsert_per_method_and_kind() {
        let s = store();
        s.save_app(&app("a")).unwrap();
        let mut r = Rule {
            app_id: "a".into(),
            method: Method::SignEvent,
            kind: None,
            allow: true,
            until: None,
            created_at: 1,
        };
        s.put_rule(&r).unwrap();
        r.allow = false;
        s.put_rule(&r).unwrap();
        r.kind = Some(7);
        s.put_rule(&r).unwrap();
        let rules = s.rules("a").unwrap();
        assert_eq!(rules.len(), 2);
        assert!(rules.iter().all(|r| !r.allow));
        s.delete_rule("a", &Method::SignEvent, None).unwrap();
        assert_eq!(s.rules("a").unwrap().len(), 1);
    }

    #[test]
    fn activity_log() {
        let s = store();
        for (i, allowed) in [true, false, true].into_iter().enumerate() {
            s.log_activity(&ActivityEntry {
                id: 0,
                at: 100 + i as u64,
                app_id: "a".into(),
                app_name: "App".into(),
                account: "pk".into(),
                method: Method::SignEvent,
                kind: Some(1),
                kind_label: None,
                allowed,
                source: Source::User,
                reason: None,
            })
            .unwrap();
        }
        let all = s.activity(&ActivityQuery::default()).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].at, 102, "newest first");
        assert_eq!(all[0].kind_label.as_deref(), Some("Short note"));
        assert_eq!(all[0].source, Source::User);
        let denied = s
            .activity(&ActivityQuery {
                allowed: Some(false),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(denied.len(), 1);
        assert_eq!(
            s.activity_stats(101).unwrap(),
            ActivityStats {
                allowed: 1,
                denied: 1
            }
        );
        assert_eq!(s.prune_activity(102).unwrap(), 2);
    }

    #[test]
    fn seen_requests() {
        let s = store();
        assert!(s.mark_seen("x", 1).unwrap());
        assert!(!s.mark_seen("x", 2).unwrap());
        assert_eq!(s.prune_seen(5).unwrap(), 1);
        assert!(s.mark_seen("x", 6).unwrap());
    }
}
