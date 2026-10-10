//! `peekd.log`, rotated at 10 MB × 3 (BLUEPRINT §1.7). The CLI's
//! `ensure_service` shows its last line when peekd fails to start, so every
//! line is self-contained and never carries a token.

use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    os::unix::fs::OpenOptionsExt as _,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
};

use tracing_subscriber::{EnvFilter, fmt::MakeWriter};

/// Rotate when the live file would exceed this.
pub const MAX_LOG_BYTES: u64 = 10 * 1024 * 1024;
/// Files kept: `peekd.log`, `peekd.log.1`, `peekd.log.2`.
pub const KEEP_FILES: usize = 3;

#[derive(Debug)]
struct Inner {
    file: File,
    len: u64,
}

/// A size-rotated log file.
#[derive(Clone, Debug)]
pub struct RotatingLog {
    path: PathBuf,
    max_bytes: u64,
    keep: usize,
    inner: Arc<Mutex<Inner>>,
}

fn open_append(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
}

impl RotatingLog {
    /// Opens (appending) `path` with the default limits.
    ///
    /// # Errors
    /// I/O failures.
    pub fn open(path: &Path) -> io::Result<Self> {
        Self::with_limits(path, MAX_LOG_BYTES, KEEP_FILES)
    }

    /// Opens with explicit limits.
    ///
    /// # Errors
    /// I/O failures.
    pub fn with_limits(path: &Path, max_bytes: u64, keep: usize) -> io::Result<Self> {
        let file = open_append(path)?;
        let len = file.metadata()?.len();
        Ok(Self {
            path: path.to_path_buf(),
            max_bytes,
            keep: keep.max(1),
            inner: Arc::new(Mutex::new(Inner { file, len })),
        })
    }

    fn rotated(&self, n: usize) -> PathBuf {
        let mut s = self.path.as_os_str().to_owned();
        s.push(format!(".{n}"));
        PathBuf::from(s)
    }

    fn rotate(&self, inner: &mut Inner) -> io::Result<()> {
        inner.file.flush()?;
        for n in (1..self.keep).rev() {
            let from = if n == 1 {
                self.path.clone()
            } else {
                self.rotated(n - 1)
            };
            if from.exists() {
                std::fs::rename(&from, self.rotated(n))?;
            }
        }
        if self.keep == 1 {
            std::fs::remove_file(&self.path).ok();
        }
        inner.file = open_append(&self.path)?;
        inner.len = 0;
        Ok(())
    }

    fn write_record(&self, buf: &[u8]) -> io::Result<usize> {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let len = u64::try_from(buf.len()).unwrap_or(u64::MAX);
        if inner.len > 0 && inner.len.saturating_add(len) > self.max_bytes {
            self.rotate(&mut inner)?;
        }
        inner.file.write_all(buf)?;
        inner.len = inner.len.saturating_add(len);
        Ok(buf.len())
    }
}

/// One event's writer.
#[derive(Debug)]
pub struct LogWriter {
    log: RotatingLog,
}

impl Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.log.write_record(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut inner = self
            .log
            .inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        inner.file.flush()
    }
}

impl<'a> MakeWriter<'a> for RotatingLog {
    type Writer = LogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        LogWriter { log: self.clone() }
    }
}

/// Installs the global subscriber: `peekd.log` when `file` is set, else
/// stderr. The filter comes from `PEEKD_LOG` (default `info`).
///
/// # Errors
/// I/O failures opening the file. A subscriber already installed (tests) is
/// not an error.
pub fn init(file: Option<&Path>) -> io::Result<()> {
    let filter = EnvFilter::try_from_env("PEEKD_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_target(false);
    let result = match file {
        Some(path) => builder.with_writer(RotatingLog::open(path)?).try_init(),
        None => builder.with_writer(io::stderr).try_init(),
    };
    // A second init (e.g. several daemons in one test process) keeps the first.
    let _ = result;
    Ok(())
}
