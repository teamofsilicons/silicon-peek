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
//!
//! Schema 2 (peek 0.1.2: always-queue FIFO, expiry on every send, scheduling)
//! adds `sends.expires_at/queued_at/overflow/schedule_id/due_at` and the
//! `scheduled` table ([`SCHEMA_V2`]); `asks.state` may also be `replaced`.

use std::{
    path::Path,
    sync::{Arc, Mutex, PoisonError},
};

use rusqlite::Connection;
use silicon_peek_client::{Error, ErrorCode, Result};

/// The schema version this build writes (`PRAGMA user_version`).
pub const SCHEMA_VERSION: i64 = 3;

const SCHEMA_V1: &str = r#"
CREATE TABLE homes (
  home_path    TEXT PRIMARY KEY,          -- canonical <SILICON_HOME>/.peek
  token_sha256 BLOB NOT NULL,             -- sha256(daemon-token); never the token
  api_url      TEXT NOT NULL,             -- last slot seen
  context      TEXT NOT NULL,
  account_id       TEXT,
  actor_id     TEXT,
  attached_at  INTEGER NOT NULL,
  last_seen_at INTEGER NOT NULL,
  config       TEXT                       -- config.sync mirror (voice, language, notify, telemetry)
);
CREATE TABLE slots (
  context       TEXT NOT NULL,
  slot          INTEGER NOT NULL CHECK (slot BETWEEN 1 AND 8),
  account_id        TEXT NOT NULL,
  actor_id      TEXT NOT NULL,
  home_path     TEXT NOT NULL,
  registered_at INTEGER NOT NULL,
  api_url       TEXT NOT NULL,            -- the slot's backend
  display_name  TEXT,
  PRIMARY KEY (context, slot),
  UNIQUE (context, account_id)
);
CREATE TABLE drawings (
  context       TEXT NOT NULL,
  account_id        TEXT NOT NULL,
  actor_id      TEXT NOT NULL,
  sha256        TEXT NOT NULL,
  path          TEXT NOT NULL,
  bytes         INTEGER NOT NULL,
  active_since  INTEGER NOT NULL,
  server_sync   TEXT NOT NULL,            -- pending|synced|skipped|failed
  last_error    TEXT,                     -- runtime failure (drawing.error) as JSON
  error_pending INTEGER NOT NULL DEFAULT 0, -- not yet attached to a CLI result
  previous_path TEXT,                     -- kept for rollback
  PRIMARY KEY (context, account_id)
);
CREATE TABLE sends (
  send_id        TEXT PRIMARY KEY,
  context        TEXT NOT NULL,
  account_id         TEXT NOT NULL,
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
CREATE INDEX sends_actor ON sends(context, account_id, actor_id, send_id);
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
  account_id          TEXT NOT NULL,
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
CREATE INDEX outbox_actor ON outbox(context, account_id, actor_id, status);
CREATE TABLE telemetry_outbox (
  id         TEXT PRIMARY KEY,
  table_id   TEXT NOT NULL,
  event      BLOB NOT NULL,
  created_at INTEGER NOT NULL,
  source     TEXT NOT NULL DEFAULT 'daemon' -- daemon|cli|mac (X-Peek-Source)
);

"#;

/// Schema 2 (peek 0.1.2): queue v2, expiry on every send, scheduling.
pub const SCHEMA_V2: &str = r"
ALTER TABLE sends ADD COLUMN expires_at  INTEGER;                    -- unix ms; every kind (asks also keep asks.expires_at)
ALTER TABLE sends ADD COLUMN queued_at   INTEGER;                    -- unix ms it entered its queue (created_at, or fire time)
ALTER TABLE sends ADD COLUMN overflow    INTEGER NOT NULL DEFAULT 0; -- 1 = due scheduled send waiting for a free spot
ALTER TABLE sends ADD COLUMN schedule_id TEXT;                       -- sch_… when it came from --in/--at
ALTER TABLE sends ADD COLUMN due_at      INTEGER;                    -- unix ms, scheduled sends
UPDATE sends SET queued_at = created_at WHERE queued_at IS NULL;
UPDATE sends SET expires_at = (SELECT a.expires_at FROM asks a WHERE a.send_id = sends.send_id)
 WHERE expires_at IS NULL AND closed_at IS NULL;
CREATE INDEX sends_expiry ON sends(closed_at, expires_at);
CREATE INDEX sends_schedule ON sends(schedule_id);
CREATE TABLE scheduled (
  schedule_id TEXT PRIMARY KEY,           -- sch_<uuidv7>
  send_id     TEXT NOT NULL UNIQUE,       -- pre-assigned snd_ (becomes sends.send_id when it fires)
  ask_id      TEXT UNIQUE,                -- pre-assigned ask_ for --ask
  context     TEXT NOT NULL,
  account_id      TEXT NOT NULL,
  actor_id    TEXT NOT NULL,
  home_path   TEXT NOT NULL,
  api_url     TEXT NOT NULL,
  isi         TEXT,
  payload     BLOB NOT NULL,              -- SendPayload JSON; image paths are cache paths (copied at schedule time)
  notify      TEXT NOT NULL,              -- JSON array
  kind        TEXT NOT NULL,
  replace_current INTEGER NOT NULL DEFAULT 0, -- --replace (not named `replace`: an SQL keyword)
  due_at      INTEGER NOT NULL,           -- unix ms
  expires_at  INTEGER,                    -- unix ms, absolute
  tz          TEXT,                       -- IANA name used for --at (display only)
  warnings    TEXT,                       -- JSON array returned at schedule time
  created_at  INTEGER NOT NULL
);
CREATE INDEX scheduled_due ON scheduled(due_at, schedule_id);
CREATE INDEX scheduled_actor ON scheduled(context, account_id, actor_id, due_at);
";

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
    if (1..3).contains(&version) {
        // Organization authority cannot be mapped to personal Accounts UUIDs.
        // Keep an exact SQLite snapshot before starting the new account store.
        let archive = path.with_extension("pre-accounts.sqlite");
        if !archive.exists() {
            conn.execute("VACUUM INTO ?1", [archive.to_string_lossy().as_ref()])
                .sql()?;
        }
        conn.pragma_update(None, "foreign_keys", "OFF").sql()?;
        let reset = conn.execute_batch(&format!(
            "BEGIN;
            DROP TABLE IF EXISTS asks; DROP TABLE IF EXISTS scheduled;
            DROP TABLE IF EXISTS sends; DROP TABLE IF EXISTS outbox;
            DROP TABLE IF EXISTS drawings; DROP TABLE IF EXISTS slots;
            DROP TABLE IF EXISTS homes; DROP TABLE IF EXISTS telemetry_outbox;
            DROP TABLE IF EXISTS cli_watchdog;
            {SCHEMA_V1} {SCHEMA_V2} PRAGMA user_version=3; COMMIT;"
        ));
        if reset.is_err() {
            let _ = conn.execute_batch("ROLLBACK;");
        }
        conn.pragma_update(None, "foreign_keys", "ON").sql()?;
        reset.sql()?;
        tracing::info!(archive=%archive.display(),"Preserved legacy Peek data; sign in with ACCOUNTS to continue");
    } else if version == 0 {
        conn.execute_batch(&format!(
            "BEGIN; {SCHEMA_V1} {SCHEMA_V2} PRAGMA user_version=3; COMMIT;"
        ))
        .sql()?;
    }
    Ok(())
}
