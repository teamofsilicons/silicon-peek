//! `peekd.sqlite` (BLUEPRINT §1.7): schema, migrations and a small async
//! handle that runs every statement on tokio's blocking pool.
//!
//! Every row is keyed by `context`; production and testing data never mix.
//! The database holds **no tokens**. All times are unix **milliseconds**.
//! The additive columns beyond the blueprint (`homes.config`,
//! `slots.api_url/display_name/testing`, `drawings.error_pending/previous_path`,
//! `sends.home_path/api_url/kind/warnings/speech_done_at`,
//! `asks.event_id/created_at/closed_at`, `outbox.subject_id`,
//! `telemetry_outbox.source`, table `cli_watchdog`) are documented inline.

use std::{
    path::Path,
    sync::{Arc, Mutex, PoisonError},
};

use rusqlite::Connection;
use silicon_peek_client::{Error, ErrorCode, Result};

/// The schema version this build writes (`PRAGMA user_version`).
pub const SCHEMA_VERSION: i64 = 1;

const SCHEMA_V1: &str = r#"
CREATE TABLE homes (
  home_path    TEXT PRIMARY KEY,          -- canonical <SILICON_HOME>/.peek
  token_sha256 BLOB NOT NULL,             -- sha256(daemon-token); never the token
  api_url      TEXT NOT NULL,             -- last slot seen
  context      TEXT NOT NULL,
  org_id       TEXT,
  actor_id     TEXT,
  attached_at  INTEGER NOT NULL,
  last_seen_at INTEGER NOT NULL,
  config       TEXT                       -- config.sync mirror (voice, language, notify, telemetry)
);
CREATE TABLE slots (
  context       TEXT NOT NULL,
  slot          INTEGER NOT NULL CHECK (slot BETWEEN 1 AND 8),
  org_id        TEXT NOT NULL,
  actor_id      TEXT NOT NULL,
  home_path     TEXT NOT NULL,
  registered_at INTEGER NOT NULL,
  api_url       TEXT NOT NULL,            -- the slot's backend
  display_name  TEXT,
  testing       TEXT,                     -- {"id","name","generation"} for the TEST pill
  PRIMARY KEY (context, slot),
  UNIQUE (context, org_id, actor_id)
);
CREATE TABLE drawings (
  context       TEXT NOT NULL,
  org_id        TEXT NOT NULL,
  actor_id      TEXT NOT NULL,
  sha256        TEXT NOT NULL,
  path          TEXT NOT NULL,
  bytes         INTEGER NOT NULL,
  active_since  INTEGER NOT NULL,
  server_sync   TEXT NOT NULL,            -- pending|synced|skipped|failed
  last_error    TEXT,                     -- runtime failure (drawing.error) as JSON
  error_pending INTEGER NOT NULL DEFAULT 0, -- not yet attached to a CLI result
  previous_path TEXT,                     -- kept for rollback
  PRIMARY KEY (context, org_id, actor_id)
);
CREATE TABLE sends (
  send_id        TEXT PRIMARY KEY,
  context        TEXT NOT NULL,
  org_id         TEXT NOT NULL,
  actor_id       TEXT NOT NULL,
  slot           INTEGER NOT NULL,
  isi            TEXT,
  payload        BLOB NOT NULL,           -- what the bubble shows (image paths are cache paths)
  notify         TEXT NOT NULL,           -- JSON array
  created_at     INTEGER NOT NULL,
  shown_at       INTEGER,
  closed_at      INTEGER,
  close_reason   TEXT,
  home_path      TEXT NOT NULL,
  api_url        TEXT NOT NULL,
  kind           TEXT NOT NULL,           -- speak|show|ask joined with '+'
  warnings       TEXT,                    -- JSON array of {code,message}
  speech_done_at INTEGER
);
CREATE INDEX sends_actor ON sends(context, org_id, actor_id, send_id);
CREATE INDEX sends_open ON sends(closed_at, created_at);
CREATE TABLE asks (
  ask_id        TEXT PRIMARY KEY,
  send_id       TEXT NOT NULL REFERENCES sends,
  state         TEXT NOT NULL,            -- pending|answered|dismissed|expired|cancelled
  answer        BLOB,
  via           TEXT,
  transcript    TEXT,
  answered_at   INTEGER,
  expires_at    INTEGER,
  waiter        INTEGER NOT NULL DEFAULT 0,
  delivered_via TEXT,                     -- ting|wait
  event_id      TEXT,                     -- the outbox row that carries its ting
  created_at    INTEGER NOT NULL,
  closed_at     INTEGER
);
CREATE INDEX asks_state ON asks(state, expires_at);
CREATE INDEX asks_send ON asks(send_id);
CREATE TABLE outbox (
  event_id        TEXT PRIMARY KEY,
  context         TEXT NOT NULL,
  home_path       TEXT NOT NULL,
  api_url         TEXT NOT NULL,
  org_id          TEXT NOT NULL,
  actor_id        TEXT NOT NULL,
  kind            TEXT NOT NULL,          -- ting|drawing.put|drawing.delete
  request         BLOB NOT NULL,          -- exact bytes POSTed; never re-serialized
  created_at      INTEGER NOT NULL,
  next_attempt_at INTEGER NOT NULL,
  attempts        INTEGER NOT NULL DEFAULT 0,
  status          TEXT NOT NULL,          -- pending|authority_required|accepted|expired|cancelled|failed
  last_error_code TEXT,
  last_request_id TEXT,
  ting_id         TEXT,
  silent          INTEGER,
  accepted_at     INTEGER,
  subject_id      TEXT                    -- ask_/snd_/cmsg_ the row reports on
);
CREATE INDEX outbox_due   ON outbox(status, next_attempt_at);
CREATE INDEX outbox_actor ON outbox(context, org_id, actor_id, status);
CREATE TABLE telemetry_outbox (
  id         TEXT PRIMARY KEY,
  table_id   TEXT NOT NULL,
  event      BLOB NOT NULL,
  created_at INTEGER NOT NULL,
  source     TEXT NOT NULL DEFAULT 'daemon' -- daemon|cli|mac (X-Peek-Source)
);
CREATE TABLE cli_watchdog (
  context_dir  TEXT PRIMARY KEY,          -- a Honeycomb context directory holding peek
  behind_since INTEGER NOT NULL,
  last_run_at  INTEGER,
  last_outcome TEXT
);
"#;

/// Converts rusqlite errors into peek errors.
pub trait SqlResult<T> {
    /// Maps the error to `internal_error` naming the database.
    ///
    /// # Errors
    /// The mapped error.
    fn sql(self) -> Result<T>;
}

impl<T> SqlResult<T> for rusqlite::Result<T> {
    fn sql(self) -> Result<T> {
        self.map_err(|e| {
            Error::new(
                ErrorCode::InternalError,
                format!("peekd's database (peekd.sqlite) failed: {e}"),
            )
            .with_hint("if this repeats, quit Peek, move ~/Library/Application Support/Peek/peekd.sqlite aside and reopen Peek")
        })
    }
}

/// A shared connection, used from async code through [`Db::call`].
#[derive(Clone, Debug)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

impl Db {
    /// Opens (creating 0600) and migrates the database.
    ///
    /// # Errors
    /// `internal_error` when it cannot be opened, or `store_schema_newer`
    /// when a newer peekd wrote it.
    pub fn open(path: &Path) -> Result<Self> {
        if !path.exists() {
            // Create it private before SQLite opens it (umask is 077 in
            // production; tests may run with a wider mask).
            silicon_peek_client::runtime::fs::create_exclusive(path, b"")?;
        }
        let conn = Connection::open(path).sql()?;
        conn.pragma_update(None, "journal_mode", "WAL").sql()?;
        conn.pragma_update(None, "synchronous", "NORMAL").sql()?;
        conn.pragma_update(None, "foreign_keys", "ON").sql()?;
        conn.busy_timeout(std::time::Duration::from_secs(5)).sql()?;
        migrate(&conn, path)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Runs `f` with the connection on the blocking pool.
    ///
    /// # Errors
    /// Whatever `f` returns, or `internal_error` if the task panicked.
    pub async fn call<T, F>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let mut guard = conn.lock().unwrap_or_else(PoisonError::into_inner);
            f(&mut guard)
        })
        .await
        .map_err(|e| Error::internal(format!("a database task failed: {e}")))?
    }

    /// Runs `f` inside one transaction (committed when `f` returns `Ok`).
    ///
    /// # Errors
    /// As [`Db::call`]; the transaction rolls back on error.
    pub async fn tx<T, F>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&rusqlite::Transaction<'_>) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        self.call(move |c| {
            let tx = c.transaction().sql()?;
            let out = f(&tx)?;
            tx.commit().sql()?;
            Ok(out)
        })
        .await
    }
}

fn migrate(conn: &Connection, path: &Path) -> Result<()> {
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .sql()?;
    if version > SCHEMA_VERSION {
        return Err(Error::new(
            ErrorCode::StoreSchemaNewer,
            format!(
                "{} uses schema {version}, but this peekd ({}) understands schema {SCHEMA_VERSION}; a newer Peek wrote it",
                path.display(),
                silicon_peek_client::VERSION
            ),
        )
        .with_hint("reinstall the newest Peek.app (peek app install), or move peekd.sqlite aside"));
    }
    if version < 1 {
        conn.execute_batch(&format!(
            "BEGIN; {SCHEMA_V1} PRAGMA user_version = 1; COMMIT;"
        ))
        .sql()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    #[tokio::test]
    async fn opens_migrates_and_refuses_newer() -> Result<()> {
        let dir = tempfile::tempdir().map_err(|e| Error::internal(e.to_string()))?;
        let path = dir.path().join("peekd.sqlite");
        let db = Db::open(&path)?;
        let n: i64 = db
            .call(|c| {
                c.query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table'",
                    [],
                    |r| r.get(0),
                )
                .sql()
            })
            .await?;
        assert_eq!(n, 8);
        drop(db);
        // Re-open is idempotent.
        let db = Db::open(&path)?;
        db.call(|c| c.execute("PRAGMA user_version = 99", []).sql())
            .await?;
        drop(db);
        let e = Db::open(&path).err();
        assert!(e.is_some_and(|e| *e.code() == ErrorCode::StoreSchemaNewer));
        let mode = std::fs::metadata(&path)
            .map_err(|e| Error::internal(e.to_string()))?
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0);
        Ok(())
    }
}
