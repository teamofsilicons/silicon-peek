//! SQLite access: one connection per database file, WAL mode, embedded
//! migrations applied at startup.
//!
//! Statements run on the blocking pool behind a mutex. peek-server's write
//! volume is tiny (one row per delivery, drawing or report), so a single
//! serialized connection per file is both simplest and safe.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use rusqlite::{Connection, OpenFlags};

use crate::error::{ApiError, ApiResult};

/// Embedded migrations, applied in order; `PRAGMA user_version` records the
/// last one applied.
const MIGRATIONS: &[(i64, &str)] = &[
    (1, include_str!("../migrations/0001_initial.sql")),
    (
        2,
        include_str!("../migrations/0002_iam5_feature_consent.sql"),
    ),
    (3, include_str!("../migrations/0003_silicon_accounts.sql")),
];

/// Why a database could not be opened.
#[derive(Debug, thiserror::Error)]
pub(crate) enum OpenError {
    /// The parent directory could not be created.
    #[error("cannot create the directory for {path}: {source}")]
    Directory {
        path: PathBuf,
        source: std::io::Error,
    },
    /// SQLite refused.
    #[error("cannot open SQLite database {path}: {source}")]
    Sqlite {
        path: PathBuf,
        source: rusqlite::Error,
    },
    /// The file was migrated by a newer peek-server.
    #[error(
        "database {path} has schema version {found}, newer than this peek-server supports ({supported}); deploy the newer build or restore a backup"
    )]
    Newer {
        path: PathBuf,
        found: i64,
        supported: i64,
    },
}

/// A database handle. Cheap to clone.
#[derive(Clone)]
pub(crate) struct Db {
    conn: Arc<Mutex<Connection>>,
}

impl Db {
    /// Opens (creating if needed) and migrates the database at `path`.
    pub(crate) fn open(path: &Path) -> Result<Self, OpenError> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|source| OpenError::Directory {
                path: path.to_owned(),
                source,
            })?;
        }
        let sqlite = |source| OpenError::Sqlite {
            path: path.to_owned(),
            source,
        };
        let mut conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(sqlite)?;
        conn.busy_timeout(Duration::from_secs(5)).map_err(sqlite)?;
        let mode: String = conn
            .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
            .map_err(sqlite)?;
        if !mode.eq_ignore_ascii_case("wal") {
            tracing::warn!(path = %path.display(), mode, "SQLite did not switch to WAL mode");
        }
        // Feature refresh retry identities must survive before the upstream rotation.
        conn.pragma_update(None, "synchronous", "FULL")
            .map_err(sqlite)?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(sqlite)?;
        migrate(&mut conn, path)?;
        restrict_permissions(path);
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Runs `f` with the connection on the blocking pool.
    pub(crate) async fn call<T, F>(&self, f: F) -> ApiResult<T>
    where
        F: FnOnce(&mut Connection) -> ApiResult<T> + Send + 'static,
        T: Send + 'static,
    {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let mut guard = conn
                .lock()
                .map_err(|_| ApiError::internal("the database connection lock is poisoned"))?;
            f(&mut guard)
        })
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "database task failed");
            ApiError::internal("a database task failed")
        })?
    }

    /// `SELECT 1`, for readiness.
    pub(crate) async fn ping(&self) -> ApiResult<()> {
        self.call(|conn| {
            conn.query_row("SELECT 1", [], |row| row.get::<_, i64>(0))?;
            Ok(())
        })
        .await
    }
}

fn migrate(conn: &mut Connection, path: &Path) -> Result<(), OpenError> {
    let sqlite = |source| OpenError::Sqlite {
        path: path.to_owned(),
        source,
    };
    let current: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(sqlite)?;
    let latest = MIGRATIONS.last().map_or(0, |(v, _)| *v);
    if current > latest {
        return Err(OpenError::Newer {
            path: path.to_owned(),
            found: current,
            supported: latest,
        });
    }
    for (version, sql) in MIGRATIONS.iter().filter(|(v, _)| *v > current) {
        let tx = conn.transaction().map_err(sqlite)?;
        tx.execute_batch(sql).map_err(sqlite)?;
        tx.pragma_update(None, "user_version", version)
            .map_err(sqlite)?;
        tx.commit().map_err(sqlite)?;
        tracing::info!(path = %path.display(), version, "applied database migration");
    }
    Ok(())
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
        tracing::warn!(path = %path.display(), error = %e, "could not restrict database permissions to 0600");
    }
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}
