//! The start sequence (BLUEPRINT §1.8) and the running daemon's handle.
//!
//! 1. (`main`) `umask(0o077)`.
//! 2. Take `peekd.lock` (`LOCK_EX|LOCK_NB`; the loser fails with
//!    `daemon_running`), remove a stale socket only after that, bind 0600.
//! 3. Open and migrate `peekd.sqlite`.
//! 4. Load slots, drawings and queues.
//! 5. If the UI is absent and a Silicon has a pending ask or queued send,
//!    open Peek.app.
//! 6. Start the outbox worker, timers, the telemetry relay and the updater.
//! 7. On shutdown: stop accepting, finish in-flight writes, remove the
//!    socket, release the lock.

use std::{
    collections::HashMap,
    fs::File,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Instant,
};

use silicon_peek_client::{
    Error, ErrorCode, Result,
    runtime::{daemon::ensure_socket_dir, fs::open_lock_file},
};
use tokio::{net::UnixListener, sync::watch, task::JoinHandle};

use crate::{
    config::DaemonConfig,
    db::Db,
    net::Net,
    paths::Paths,
    settings::SettingsStore,
    speech::Speech,
    state::{Core, Shared, SharedRef},
    telemetry::{Record, Telemetry},
    ui::UiHub,
};

/// A running daemon.
#[derive(Debug)]
pub struct DaemonHandle {
    shared: SharedRef,
    tasks: Vec<JoinHandle<()>>,
    _lock: File,
    socket_path: PathBuf,
}

fn take_lock(path: &Path) -> Result<File> {
    let file = open_lock_file(path)?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => Err(Error::new(
            ErrorCode::DaemonRunning,
            format!(
                "another peekd already holds {} for this account",
                path.display()
            ),
        )
        .with_hint(
            "use the running peekd (`peek daemon status`); `peek daemon restart` replaces it",
        )),
        Err(std::fs::TryLockError::Error(e)) => Err(Error::internal(format!(
            "locking {} failed: {e}",
            path.display()
        ))),
    }
}

fn bind(path: &Path) -> Result<UnixListener> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => std::fs::remove_file(path).map_err(|e| {
            Error::internal(format!(
                "removing the stale socket {} failed: {e}",
                path.display()
            ))
        })?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(Error::internal(format!(
                "inspecting {} failed: {e}",
                path.display()
            )));
        }
    }
    let listener = UnixListener::bind(path).map_err(|e| {
        Error::new(
            ErrorCode::DaemonUnavailable,
            format!("binding the peekd socket {} failed: {e}", path.display()),
        )
    })?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| Error::internal(format!("restricting {} failed: {e}", path.display())))?;
    Ok(listener)
}

/// Recordings outlive their rows only by accident (a crash); none is needed
/// past the longest delivery window (168 h), so older ones are deleted.
fn sweep_recordings(dir: &Path) {
    let max_age = std::time::Duration::from_hours(8 * 24);
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| m.elapsed().ok())
            .is_some_and(|age| age > max_age);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Starts a daemon with `cfg` (steps 2–6 of §1.8).
///
/// # Errors
/// `daemon_running` when another instance holds the lock;
/// `daemon_identity_mismatch` for an unusable socket directory; database and
/// filesystem failures.
pub async fn start(cfg: DaemonConfig) -> Result<DaemonHandle> {
    let socket_dir = cfg
        .socket_path
        .parent()
        .ok_or_else(|| Error::internal("the socket path has no directory"))?
        .to_path_buf();
    ensure_socket_dir(&socket_dir)?;
    let lock = take_lock(&cfg.lock_path)?;
    let listener = bind(&cfg.socket_path)?;

    let paths = Paths::new(&cfg.support_dir);
    paths.ensure()?;
    let db = Db::open(&paths.db())?;
    let settings = SettingsStore::load(paths.settings());
    let (shutdown, _) = watch::channel(false);
    let shared = Arc::new(Shared {
        telemetry: Telemetry::new(&cfg.bundle_id),
        speech: Speech::new(paths.tts_dir())?,
        net: Net::new()?,
        settings,
        db,
        paths,
        ui: UiHub::default(),
        core: tokio::sync::Mutex::new(Core::default()),
        waiters: Mutex::new(HashMap::new()),
        outbox_wake: tokio::sync::Notify::new(),
        timers_wake: tokio::sync::Notify::new(),
        update_wake: tokio::sync::Notify::new(),
        update_swapping: std::sync::atomic::AtomicBool::new(false),
        shutdown,
        exit_request: Mutex::new(None),
        prewarmed: Mutex::new(HashMap::new()),
        last_launch: Mutex::new(None),
        instance_id: uuid::Uuid::now_v7().hyphenated().to_string(),
        started: Instant::now(),
        cfg,
    });
    let queued = shared.load_queues().await?;
    shared.load_telemetry_mirror().await?;
    shared.speech.cache.prune();
    sweep_recordings(&shared.paths.recordings_dir());
    shared.prune_image_cache().await;
    tracing::info!(
        version = silicon_peek_client::VERSION,
        socket = %shared.cfg.socket_path.display(),
        queued,
        "peekd started"
    );
    shared.record(Record::new("daemon.started", "ok"));

    let mut tasks = vec![
        tokio::spawn(crate::server::serve(Arc::clone(&shared), listener)),
        tokio::spawn(Arc::clone(&shared).run_outbox()),
        tokio::spawn(Arc::clone(&shared).run_timers()),
        tokio::spawn(Arc::clone(&shared).run_telemetry()),
    ];
    if shared.cfg.updates_enabled {
        tasks.push(tokio::spawn(Arc::clone(&shared).run_updates()));
        tasks.push(tokio::spawn(Arc::clone(&shared).run_maintenance()));
    }
    if shared.cfg.launch_ui {
        let s = Arc::clone(&shared);
        tasks.push(tokio::spawn(async move {
            tokio::time::sleep(s.cfg.timings.launch_grace).await;
            if !s.ui.is_connected() && s.has_pending_work().await {
                s.launch_ui_soon();
            }
        }));
    }
    let socket_path = shared.cfg.socket_path.clone();
    Ok(DaemonHandle {
        shared,
        tasks,
        _lock: lock,
        socket_path,
    })
}

impl DaemonHandle {
    /// The socket clients connect to.
    #[must_use]
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// `~/Library/Application Support/Peek` of this daemon.
    #[must_use]
    pub fn support_dir(&self) -> &Path {
        &self.shared.paths.support
    }

    /// Resolves when a component asks the daemon to exit (the updater after
    /// swapping the app) or shutdown was requested.
    pub async fn exit_requested(&self) {
        let mut rx = self.shared.shutdown.subscribe();
        while !*rx.borrow() {
            if rx.changed().await.is_err() {
                return;
            }
        }
    }

    /// Runs one outbox pass now (tests and `peek daemon` diagnostics).
    ///
    /// # Errors
    /// Database failures.
    pub async fn deliver_now(&self) -> Result<usize> {
        self.shared.outbox_pass().await
    }

    /// Runs one telemetry relay pass now.
    ///
    /// # Errors
    /// Database failures.
    pub async fn relay_telemetry_now(&self) -> Result<usize> {
        self.shared.relay_telemetry_once().await
    }

    /// Runs one update attempt now.
    ///
    /// # Errors
    /// As `update_once`.
    pub async fn update_now(&self) -> Result<crate::update::UpdateOutcome> {
        self.shared.update_once().await
    }

    /// Runs the CLI watchdog now.
    ///
    /// # Errors
    /// Database failures.
    pub async fn watchdog_now(&self) -> Result<usize> {
        self.shared.cli_watchdog().await
    }

    /// Whether Peek.app is connected.
    #[must_use]
    pub fn ui_connected(&self) -> bool {
        self.shared.ui.is_connected()
    }

    /// Stops the daemon gracefully and returns the exit status.
    pub async fn shutdown(self) -> i32 {
        // `send_replace`: tasks that have not subscribed yet still see it.
        self.shared.shutdown.send_replace(true);
        for t in self.tasks {
            let abort = t.abort_handle();
            if tokio::time::timeout(std::time::Duration::from_secs(5), t)
                .await
                .is_err()
            {
                abort.abort();
            }
        }
        self.shared.speech.cancel_all();
        let _ = std::fs::remove_file(&self.socket_path);
        let code = self
            .shared
            .exit_request
            .lock()
            .ok()
            .and_then(|g| *g)
            .unwrap_or(0);
        tracing::info!(
            code,
            uptime_s = self.shared.started.elapsed().as_secs(),
            "peekd stopped"
        );
        code
    }
}
