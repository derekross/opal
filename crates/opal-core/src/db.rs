//! One SQLite database shared by all modules. Each module owns its tables and
//! migrates them itself through [`Db::migrate`].

use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::Connection;

use crate::{Error, Result, paths};

#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

impl Db {
    pub fn open_default() -> Result<Self> {
        let dir = paths::data_dir();
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&dir)?;
        }
        Self::open(&dir.join("opal.db"))
    }

    pub fn open(path: &Path) -> Result<Self> {
        // Create the file private before SQLite opens it (-wal/-shm copy its mode).
        {
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .mode(0o600)
                .open(path)?;
        }
        let conn = Connection::open(path).map_err(db_err)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory().map_err(db_err)?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS schema_versions (
                 module  TEXT PRIMARY KEY,
                 version INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS kv (
                 key   TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );",
        )
        .map_err(db_err)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Run `f` with the connection. Keep it short: this holds a lock.
    pub fn with<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T> {
        let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        f(&conn).map_err(db_err)
    }

    /// Apply `steps[n..]` for a module currently at version `n`.
    pub fn migrate(&self, module: &str, steps: &[&str]) -> Result<()> {
        let mut conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        let tx = conn.transaction().map_err(db_err)?;
        let current: usize = tx
            .query_row(
                "SELECT version FROM schema_versions WHERE module = ?1",
                [module],
                |r| r.get::<_, i64>(0),
            )
            .map(|v| v as usize)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(0),
                e => Err(e),
            })
            .map_err(db_err)?;
        for step in steps.iter().skip(current) {
            tx.execute_batch(step).map_err(db_err)?;
        }
        tx.execute(
            "INSERT INTO schema_versions (module, version) VALUES (?1, ?2)
             ON CONFLICT(module) DO UPDATE SET version = excluded.version",
            rusqlite::params![module, steps.len() as i64],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)
    }

    pub fn get_kv(&self, key: &str) -> Result<Option<String>> {
        self.with(|c| {
            c.query_row("SELECT value FROM kv WHERE key = ?1", [key], |r| r.get(0))
                .map(Some)
                .or_else(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    e => Err(e),
                })
        })
    }

    pub fn set_kv(&self, key: &str, value: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO kv (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [key, value],
            )
            .map(|_| ())
        })
    }
}

fn db_err(e: rusqlite::Error) -> Error {
    Error::Db(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_run_once_and_extend() {
        let db = Db::open_in_memory().unwrap();
        db.migrate("m", &["CREATE TABLE a (x INTEGER);"]).unwrap();
        db.migrate("m", &["CREATE TABLE a (x INTEGER);"]).unwrap();
        db.migrate(
            "m",
            &[
                "CREATE TABLE a (x INTEGER);",
                "ALTER TABLE a ADD COLUMN y TEXT;",
            ],
        )
        .unwrap();
        db.with(|c| c.execute("INSERT INTO a (x, y) VALUES (1, 'z')", []))
            .unwrap();
    }

    #[test]
    fn kv() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.get_kv("k").unwrap(), None);
        db.set_kv("k", "1").unwrap();
        db.set_kv("k", "2").unwrap();
        assert_eq!(db.get_kv("k").unwrap().as_deref(), Some("2"));
    }
}
