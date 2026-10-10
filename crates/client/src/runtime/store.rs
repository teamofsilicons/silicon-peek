//! The per-home store `${SILICON_HOME:-<passwd home>}/.peek/` (BLUEPRINT §1.7).
//!
//! | file | contents |
//! |---|---|
//! | `session.lock` | empty; exclusive `flock` for every read-modify-write; never deleted |
//! | `session.json` | [`SessionFile`] |
//! | `daemon-token` | 64 hex characters, created at the first login |
//! | `config.json` | [`Config`] |
//! | `testing.json` | [`TestingFile`] |
//! | `home` | optional pointer written by `peek config home <dir>` |
//!
//! `$SILICON_HOME/.silicon-accounts` is never touched.

use std::{
    ffi::OsStr,
    fs::{File, TryLockError},
    path::{Path, PathBuf},
    time::Duration,
};

use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

use super::{
    fs::{
        create_exclusive, ensure_private_dir, open_lock_file, read_private, verify_private_dir,
        write_atomic,
    },
    session::SessionFile,
    sys,
};
use crate::{
    Secret,
    config::{CONFIG_SCHEMA, Config},
    error::{Error, ErrorCode, Result},
    json,
};

/// The store directory name under the home.
pub const STORE_DIR: &str = ".peek";
/// The lock file.
pub const LOCK_FILE: &str = "session.lock";
/// The session file.
pub const SESSION_FILE: &str = "session.json";
/// The per-home IPC token.
pub const DAEMON_TOKEN_FILE: &str = "daemon-token";
/// The config file.
pub const CONFIG_FILE: &str = "config.json";
/// The testing environments file.
/// The `config home` pointer.
pub const HOME_POINTER_FILE: &str = "home";

/// How often an async lock attempt retries while another process holds it.
const ASYNC_LOCK_POLL: Duration = Duration::from_millis(50);

fn invalid_home(msg: impl Into<String>) -> Error {
    Error::new(ErrorCode::InvalidSiliconHome, msg).with_hint(
        "set SILICON_HOME to an existing directory owned by you (Stemcell sets it per Silicon), or unset it to use your home",
    )
}

/// Resolves the Silicon home: `SILICON_HOME` when set (it must be a non-empty
/// path to an existing directory), else the password-database home.
///
/// # Errors
/// `invalid_silicon_home`.
pub fn silicon_home(value: Option<&OsStr>) -> Result<PathBuf> {
    let base = match value {
        Some(v) if v.is_empty() => {
            return Err(invalid_home(
                "SILICON_HOME is set but empty; peek will not guess where this Silicon's state lives",
            ));
        }
        Some(v) => PathBuf::from(v),
        None => sys::real_home()?,
    };
    match std::fs::metadata(&base) {
        Ok(m) if m.is_dir() => {}
        Ok(_) => {
            return Err(invalid_home(format!(
                "SILICON_HOME {} is not a directory",
                base.display()
            )));
        }
        Err(e) => {
            return Err(invalid_home(format!(
                "SILICON_HOME {} is not usable: {e}",
                base.display()
            )));
        }
    }
    std::fs::canonicalize(&base)
        .map_err(|e| invalid_home(format!("resolving {} failed: {e}", base.display())))
}

/// Where the store lives for a Silicon home: `<home>/.peek`, unless the
/// pointer `<home>/.peek/home` names another home directory, in which case
/// `<that>/.peek`. Creates nothing.
///
/// # Errors
/// `invalid_silicon_home` for an unreadable or invalid pointer.
pub fn store_dir_for(home: &Path) -> Result<PathBuf> {
    let default = home.join(STORE_DIR);
    let pointer = default.join(HOME_POINTER_FILE);
    let Some(bytes) = read_private(&pointer)? else {
        return Ok(default);
    };
    let text = String::from_utf8(bytes)
        .map_err(|_| invalid_home(format!("{} is not UTF-8", pointer.display())))?;
    let target = PathBuf::from(text.trim());
    if !target.is_absolute() || !target.is_dir() {
        return Err(invalid_home(format!(
            "{} points at {}, which is not an existing absolute directory",
            pointer.display(),
            target.display()
        ))
        .with_hint(format!(
            "fix it with `peek config home <DIR>`, or delete {}",
            pointer.display()
        )));
    }
    Ok(target.join(STORE_DIR))
}

/// Writes the `config home` pointer in the default store of `home`, so
/// later commands use `<new_home>/.peek`. Existing files are not moved.
///
/// # Errors
/// `invalid_silicon_home` when `new_home` is not an existing directory.
pub fn set_home_pointer(home: &Path, new_home: &Path) -> Result<PathBuf> {
    let target = std::fs::canonicalize(new_home).map_err(|e| {
        invalid_home(format!(
            "{} is not an existing directory: {e}",
            new_home.display()
        ))
    })?;
    if !target.is_dir() {
        return Err(invalid_home(format!(
            "{} is not a directory",
            target.display()
        )));
    }
    let default = home.join(STORE_DIR);
    ensure_private_dir(&default)?;
    let store = target.join(STORE_DIR);
    ensure_private_dir(&store)?;
    let mut line = target.to_string_lossy().into_owned().into_bytes();
    line.push(b'\n');
    write_atomic(&default, HOME_POINTER_FILE, &line)?;
    Ok(store)
}

/// The exclusive `session.lock`, held until dropped.
#[derive(Debug)]
pub struct StoreLock {
    _file: File,
    dir: PathBuf,
}

/// An opened store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    /// Resolves the store from the environment (`SILICON_HOME`) and creates it
    /// if needed (0700).
    ///
    /// # Errors
    /// `invalid_silicon_home`.
    pub fn from_env() -> Result<Self> {
        let home = silicon_home(std::env::var_os("SILICON_HOME").as_deref())?;
        Self::open(&store_dir_for(&home)?)
    }

    /// Opens `dir`, creating it (0700) if missing. Its parent must exist.
    ///
    /// # Errors
    /// `invalid_silicon_home`.
    pub fn open(dir: &Path) -> Result<Self> {
        ensure_private_dir(dir)?;
        let dir = std::fs::canonicalize(dir)
            .map_err(|e| invalid_home(format!("resolving {} failed: {e}", dir.display())))?;
        Ok(Self { dir })
    }

    /// Opens an existing store without creating or changing anything
    /// (peekd's check 1: a real directory, owned by this user, mode `& 077 == 0`).
    ///
    /// # Errors
    /// `invalid_silicon_home`.
    pub fn open_existing(dir: &Path) -> Result<Self> {
        let canonical = std::fs::canonicalize(dir).map_err(|e| {
            invalid_home(format!("{} is not an existing store: {e}", dir.display()))
        })?;
        verify_private_dir(&canonical)?;
        Ok(Self { dir: canonical })
    }

    /// The canonical store directory (the IPC `auth.home`).
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The path of a store file.
    #[must_use]
    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// Takes the exclusive lock, blocking the thread until it is free.
    ///
    /// # Errors
    /// `invalid_silicon_home` for an unusable lock file.
    pub fn lock(&self) -> Result<StoreLock> {
        let file = open_lock_file(&self.path(LOCK_FILE))?;
        file.lock().map_err(|e| {
            Error::new(
                ErrorCode::InternalError,
                format!("locking {} failed: {e}", self.path(LOCK_FILE).display()),
            )
        })?;
        Ok(StoreLock {
            _file: file,
            dir: self.dir.clone(),
        })
    }

    /// Takes the exclusive lock without blocking the async runtime: tries,
    /// then sleeps 50 ms, until it is free.
    ///
    /// # Errors
    /// `invalid_silicon_home` for an unusable lock file.
    pub async fn lock_async(&self) -> Result<StoreLock> {
        let file = open_lock_file(&self.path(LOCK_FILE))?;
        loop {
            match file.try_lock() {
                Ok(()) => {
                    return Ok(StoreLock {
                        _file: file,
                        dir: self.dir.clone(),
                    });
                }
                Err(TryLockError::WouldBlock) => tokio::time::sleep(ASYNC_LOCK_POLL).await,
                Err(TryLockError::Error(e)) => {
                    return Err(Error::new(
                        ErrorCode::InternalError,
                        format!("locking {} failed: {e}", self.path(LOCK_FILE).display()),
                    ));
                }
            }
        }
    }

    fn check_lock(&self, lock: &StoreLock) -> Result<()> {
        if lock.dir == self.dir {
            Ok(())
        } else {
            Err(Error::internal(format!(
                "a lock on {} was used to write {}",
                lock.dir.display(),
                self.dir.display()
            )))
        }
    }

    fn read_versioned<T: DeserializeOwned>(&self, name: &str, supported: u32) -> Result<Option<T>> {
        let path = self.path(name);
        let Some(bytes) = read_private(&path)? else {
            return Ok(None);
        };
        let corrupt = |why: String| {
            Error::new(
                ErrorCode::StoreCorrupt,
                format!("{} cannot be read: {why}", path.display()),
            )
            .with_hint(format!(
                "repair or delete {} (deleting session.json logs this home out)",
                path.display()
            ))
        };
        let value = json::parse_value(&bytes).map_err(|e| corrupt(e.message().to_owned()))?;
        let schema = value
            .get("schema")
            .and_then(Value::as_u64)
            .ok_or_else(|| corrupt("it has no numeric `schema`".into()))?;
        if schema > u64::from(supported) {
            return Err(Error::new(
                ErrorCode::StoreSchemaNewer,
                format!(
                    "{} uses schema {schema}, but this peek ({}) understands schema {supported}; a newer peek wrote it",
                    path.display(),
                    crate::VERSION
                ),
            )
            .with_hint("apps update 'peek'"));
        }
        serde_json::from_value(value)
            .map(Some)
            .map_err(|e| corrupt(e.to_string()))
    }

    fn write_json<T: Serialize>(&self, lock: &StoreLock, name: &str, value: &T) -> Result<()> {
        self.check_lock(lock)?;
        let mut bytes = serde_json::to_vec_pretty(value)
            .map_err(|e| Error::internal(format!("serializing {name} failed: {e}")))?;
        bytes.push(b'\n');
        write_atomic(&self.dir, name, &bytes)
    }

    /// Reads `session.json` (an empty file when absent).
    ///
    /// # Errors
    /// `store_corrupt`, `store_schema_newer`, `invalid_silicon_home`.
    pub fn read_session(&self) -> Result<SessionFile> {
        let Some(value) =
            self.read_versioned::<Value>(SESSION_FILE, super::session::SESSION_SCHEMA)?
        else {
            return Ok(SessionFile::default());
        };
        if value.get("schema").and_then(Value::as_u64).unwrap_or(1) < 2 {
            return Ok(SessionFile::default());
        }
        serde_json::from_value(value)
            .map_err(|_| Error::new(ErrorCode::StoreCorrupt, "Saved session is malformed"))
    }

    /// Atomically writes `session.json` under the lock.
    ///
    /// # Errors
    /// I/O failures.
    pub fn write_session(&self, lock: &StoreLock, session: &SessionFile) -> Result<()> {
        self.check_lock(lock)?;
        if let Some(bytes) = read_private(&self.path(SESSION_FILE))?
            && serde_json::from_slice::<Value>(&bytes)
                .ok()
                .and_then(|v| v.get("schema").and_then(Value::as_u64))
                .unwrap_or(1)
                < 2
            && !self.path("session.pre-accounts.json").exists()
        {
            write_atomic(self.dir(), "session.pre-accounts.json", &bytes)?;
        }
        let mut s = session.clone();
        s.schema = super::session::SESSION_SCHEMA;
        self.write_json(lock, SESSION_FILE, &s)
    }

    /// Locks, reads, mutates and writes `session.json` (blocking).
    ///
    /// # Errors
    /// The mutation's error (nothing is written) or I/O failures.
    pub fn update_session<T>(&self, f: impl FnOnce(&mut SessionFile) -> Result<T>) -> Result<T> {
        let lock = self.lock()?;
        let mut s = self.read_session()?;
        let out = f(&mut s)?;
        self.write_session(&lock, &s)?;
        Ok(out)
    }

    /// As [`Store::update_session`], taking the lock without blocking the
    /// async runtime.
    ///
    /// # Errors
    /// The mutation's error (nothing is written) or I/O failures.
    pub async fn update_session_async<T>(
        &self,
        f: impl FnOnce(&mut SessionFile) -> Result<T>,
    ) -> Result<T> {
        let lock = self.lock_async().await?;
        let mut s = self.read_session()?;
        let out = f(&mut s)?;
        self.write_session(&lock, &s)?;
        Ok(out)
    }

    /// Reads `config.json` (defaults when absent), validating it.
    ///
    /// # Errors
    /// `store_corrupt`, `store_schema_newer`.
    pub fn read_config(&self) -> Result<Config> {
        let c: Config = self
            .read_versioned(CONFIG_FILE, CONFIG_SCHEMA)?
            .unwrap_or_default();
        c.validate().map_err(|e| {
            Error::new(
                ErrorCode::StoreCorrupt,
                format!(
                    "{} holds an invalid value: {}",
                    self.path(CONFIG_FILE).display(),
                    e.message()
                ),
            )
            .with_hint("fix it with `peek config set '{\"<key>\":null}'` to reset the key")
        })?;
        Ok(c)
    }

    /// Atomically writes `config.json` under the lock.
    ///
    /// # Errors
    /// I/O failures.
    pub fn write_config(&self, lock: &StoreLock, config: &Config) -> Result<()> {
        let mut c = config.clone();
        c.schema = CONFIG_SCHEMA;
        self.write_json(lock, CONFIG_FILE, &c)
    }

    /// `peek config set`: parses the patch strictly, merges it under the lock
    /// and returns the resulting config.
    ///
    /// # Errors
    /// `invalid_json`, `invalid_input`, `unknown_config_key`; nothing is
    /// written on error.
    ///
    /// A stored key holding an invalid (hand-edited) value does not block the
    /// merge when the patch sets or resets that key; otherwise the error names
    /// the key and the fix.
    pub fn merge_config(&self, patch: &str) -> Result<Config> {
        let patch = Config::parse_patch(patch)?;
        let lock = self.lock()?;
        let config_path = self.path(CONFIG_FILE);
        let (mut c, invalid) = match self.read_versioned::<Value>(CONFIG_FILE, CONFIG_SCHEMA)? {
            None => (Config::default(), Vec::new()),
            Some(Value::Object(stored)) => Config::from_stored(&stored),
            Some(_) => {
                return Err(Error::new(
                    ErrorCode::StoreCorrupt,
                    format!("{} is not a JSON object", config_path.display()),
                )
                .with_hint(format!(
                    "delete {} to restore the defaults",
                    config_path.display()
                )));
            }
        };
        let unrepaired: Vec<&str> = invalid
            .iter()
            .map(String::as_str)
            .filter(|k| !patch.contains_key(*k))
            .collect();
        if let Some(first) = unrepaired.first() {
            return Err(Error::new(
                ErrorCode::StoreCorrupt,
                format!(
                    "{} holds invalid values for: {}",
                    config_path.display(),
                    unrepaired.join(", ")
                ),
            )
            .with_hint(format!(
                "include them in the same call, e.g. peek config set '{{\"{first}\":null}}' to reset"
            ))
            .with_details(serde_json::json!({"invalid_keys": unrepaired})));
        }
        c.merge(&patch)?;
        self.write_config(&lock, &c)?;
        Ok(c)
    }

    /// Reads `daemon-token`, if present.
    ///
    /// # Errors
    /// `store_corrupt` for a malformed token.
    pub fn daemon_token(&self) -> Result<Option<Secret>> {
        let path = self.path(DAEMON_TOKEN_FILE);
        let Some(bytes) = read_private(&path)? else {
            return Ok(None);
        };
        let text = String::from_utf8(bytes).unwrap_or_default();
        let token = text.trim();
        if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::new(
                ErrorCode::StoreCorrupt,
                format!("{} is not 64 hex characters", path.display()),
            )
            .with_hint(format!(
                "delete {} and run peek login again",
                path.display()
            )));
        }
        Ok(Some(Secret::new(token)))
    }

    /// Returns `daemon-token`, creating it (`O_EXCL`, 0600, 32 random bytes as
    /// hex) if absent.
    ///
    /// # Errors
    /// I/O failures or an unavailable random source.
    pub fn ensure_daemon_token(&self, lock: &StoreLock) -> Result<Secret> {
        self.check_lock(lock)?;
        if let Some(t) = self.daemon_token()? {
            return Ok(t);
        }
        let mut raw = [0u8; 32];
        getrandom::fill(&mut raw)
            .map_err(|e| Error::internal(format!("the system random source failed: {e}")))?;
        let token = hex::encode(raw);
        create_exclusive(
            &self.path(DAEMON_TOKEN_FILE),
            format!("{token}\n").as_bytes(),
        )?;
        // Another process may have won the O_EXCL race: read what is there.
        self.daemon_token()?
            .ok_or_else(|| Error::internal("daemon-token vanished right after creation"))
    }
}
