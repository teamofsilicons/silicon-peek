//! Peek.app and peekd from the CLI's side (BLUEPRINT §1.8, §4.2, §4.3).
//!
//! - [`platform_unsupported`] is the exact §4.2 error every Mac-bound command
//!   returns on Linux and Windows.
//! - On macOS, [`ensure_service`] connects to peekd (peer uid checked) and
//!   says `hello`; if nothing answers it runs `ensure_app()` (offer the
//!   bundled build, install `~/Applications/Peek.app` if absent: `ditto -x
//!   -k`, `codesign --verify --deep --strict -R=<Developer ID requirement>`,
//!   `xattr -dr com.apple.quarantine`, `renamex_np(RENAME_EXCL)`), launches
//!   it (`launchctl kickstart` when the agent is loaded, else `open -g -j`
//!   in an Aqua session, else `no_gui_session`), and polls the socket every
//!   100 ms for up to 10 s.
//! - Children get stdin `/dev/null` and stdout/stderr in a log file; they
//!   never inherit the caller's pipes (Stemcell and ISI tools read to EOF).
//!
//! Test hooks (the same names as `install-app.sh`): `PEEK_INSTALL_SUPPORT_DIR`,
//! `PEEK_INSTALL_APPLICATIONS_DIR`, and `PEEK_INSTALL_NO_LAUNCH=1`, which
//! forbids every `launchctl` and `open` call. The isolated-run variables
//! peekd and Peek.app share are honoured too: `PEEK_SUPPORT_DIR` (after
//! `PEEK_INSTALL_SUPPORT_DIR`), `PEEK_APPLICATIONS_DIR` (before
//! `PEEK_INSTALL_APPLICATIONS_DIR`) and `PEEK_NO_SERVICES=1` (same effect as
//! `PEEK_INSTALL_NO_LAUNCH=1`); `PEEK_DAEMON_SOCKET` picks the socket. peekd
//! resolves all of them in the same order.

use silicon_peek_client::{Error, platform};

/// The exact BLUEPRINT §4.2 error for a Mac-bound command on `platform`
/// (the shared definition in `silicon-peek-client`).
pub fn platform_unsupported(command: &str, platform: &str) -> Error {
    silicon_peek_client::platform_unsupported_on(command, platform)
}

/// `Ok` on macOS; the §4.2 error elsewhere.
pub fn require_mac(command: &str) -> silicon_peek_client::Result<()> {
    if cfg!(target_os = "macos") {
        Ok(())
    } else {
        Err(platform_unsupported(command, &platform()))
    }
}

#[cfg(target_os = "macos")]
pub use mac::*;

#[cfg(not(target_os = "macos"))]
pub use other::*;

#[cfg(target_os = "macos")]
mod mac {
    use std::{
        ffi::OsStr,
        fmt::Write as _,
        fs::{File, OpenOptions, TryLockError},
        io::{Read as _, Write as _},
        os::unix::fs::OpenOptionsExt as _,
        path::{Path, PathBuf},
        process::Stdio,
        time::{Duration, Instant},
    };

    use serde::{Deserialize, Serialize};
    use serde_json::{Value, json};
    use sha2::{Digest as _, Sha256};
    use silicon_peek_client::{
        Error, ErrorCode, Result,
        identity::{ApiUrl, Context},
        ipc::{
            AuthBlock, Op,
            cli::{AppOfferInfo, BundledApp, Hello, HelloResult},
        },
        platform,
        runtime::{
            daemon::{DaemonConnection, EventWait, REQUEST_TIMEOUT, socket_path},
            fs::{ensure_private_dir, open_lock_file},
            sys,
        },
        timestamp::Timestamp,
    };

    /// launchd label of peekd's agent.
    pub const AGENT_LABEL: &str = "ai.tos.peek.daemon";
    /// Release bundle identifier.
    pub const BUNDLE_ID: &str = "ai.tos.peek";
    /// Development bundle identifier (unsigned dev builds).
    pub const DEV_BUNDLE_ID: &str = "ai.tos.peek.dev";
    /// Developer ID team of release builds.
    pub const TEAM_ID: &str = "LTBSK59BJ2";

    const POLL_INTERVAL: Duration = Duration::from_millis(100);
    const POLL_BUDGET: Duration = Duration::from_secs(10);
    const UPDATE_WAIT: Duration = Duration::from_secs(30);
    const INSTALL_LOCK_WAIT: Duration = Duration::from_secs(30);
    const CHILD_TIMEOUT: Duration = Duration::from_secs(60);
    const QUERY_TIMEOUT: Duration = Duration::from_secs(10);
    const UNINSTALL_WAIT: Duration = Duration::from_secs(15);

    /// The Developer ID requirement for `team`.
    pub fn requirement(bundle_id: &str, team: &str) -> String {
        format!(
            "anchor apple generic and identifier \"{bundle_id}\" and certificate 1[field.1.2.840.113635.100.6.2.6] and certificate leaf[field.1.2.840.113635.100.6.1.13] and certificate leaf[subject.OU] = \"{team}\""
        )
    }

    /// Where Peek lives for this OS user (never under `SILICON_HOME`).
    #[derive(Clone, Debug)]
    pub struct Paths {
        /// `~/Library/Application Support/Peek`.
        pub support: PathBuf,
        /// `~/Applications`.
        pub apps: PathBuf,
        /// `~/Applications/Peek.app`.
        pub app: PathBuf,
    }

    impl Paths {
        fn offers(&self) -> PathBuf {
            self.support.join("offers")
        }
        fn log(&self) -> PathBuf {
            self.support.join("cli.log")
        }
        /// `install-status.txt`.
        pub fn install_status(&self) -> PathBuf {
            self.support.join("install-status.txt")
        }
        /// `peekd.log`.
        pub fn peekd_log(&self) -> PathBuf {
            self.support.join("peekd.log")
        }
    }

    /// Resolves [`Paths`] from the getpwuid home (or the test hooks).
    pub fn paths() -> Result<Paths> {
        let home = sys::real_home()?;
        let from_env = |k: &str| {
            std::env::var_os(k)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        let support = from_env("PEEK_INSTALL_SUPPORT_DIR")
            .or_else(|| from_env("PEEK_SUPPORT_DIR"))
            .unwrap_or_else(|| home.join("Library/Application Support/Peek"));
        // The same order as peekd: its own override, then the install hook.
        let apps = from_env("PEEK_APPLICATIONS_DIR")
            .or_else(|| from_env("PEEK_INSTALL_APPLICATIONS_DIR"))
            .unwrap_or_else(|| home.join("Applications"));
        Ok(Paths {
            app: apps.join("Peek.app"),
            support,
            apps,
        })
    }

    /// `PEEK_INSTALL_NO_LAUNCH=1` or `PEEK_NO_SERVICES=1`: never run
    /// launchctl or open (tests and isolated runs never touch the real
    /// user's launchd or `LaunchServices`).
    pub fn no_launch() -> bool {
        ["PEEK_INSTALL_NO_LAUNCH", "PEEK_NO_SERVICES"]
            .iter()
            .any(|k| std::env::var(k).is_ok_and(|v| v == "1"))
    }

    /// `Peek.app.zip` + `Peek.app.info` shipped next to this CLI.
    #[derive(Clone, Debug)]
    pub struct Payload {
        /// The package directory.
        pub pkg: PathBuf,
        /// The zip.
        pub zip: PathBuf,
        /// The parsed sidecar.
        pub info: AppOfferInfo,
    }

    /// The bundled payload: `canonicalize(current_exe)/../..`.
    pub fn bundled_payload() -> Option<Payload> {
        let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
        let pkg = exe.parent()?.parent()?.to_path_buf();
        let zip = pkg.join("Peek.app.zip");
        let info_text = std::fs::read_to_string(pkg.join("Peek.app.info")).ok()?;
        if !zip.is_file() {
            return None;
        }
        let info = AppOfferInfo::parse_sidecar(&info_text).ok()?;
        Some(Payload { pkg, zip, info })
    }

    /// The CLI's `hello`.
    pub fn hello_message() -> Hello {
        let bundled = bundled_payload().map(|p| BundledApp {
            build: p.info.bundle_version,
            short_version: p.info.short_version.clone(),
            zip_path: p.zip.to_string_lossy().into_owned(),
            zip_sha256: p.info.zip_sha256.clone(),
        });
        Hello::cli(platform(), bundled)
    }

    /// A connection to peekd that completed `hello`.
    #[derive(Debug)]
    pub struct Service {
        conn: DaemonConnection,
        /// peekd's answer to `hello`.
        pub hello: HelloResult,
    }

    impl Service {
        /// One request/reply.
        pub async fn call<O: Op>(
            &mut self,
            op: &O,
            auth: Option<&AuthBlock>,
            blobs: Vec<Vec<u8>>,
            timeout: Duration,
        ) -> Result<(O::Output, Vec<Vec<u8>>)> {
            if auth.is_some() {
                crate::commands::require_features(
                    self,
                    &[(
                        silicon_peek_client::ipc::cli::features::IAM5_CONTEXTS,
                        "saved account and organization contexts",
                    )],
                )?;
            }
            self.conn.call(op, auth, blobs, timeout).await
        }

        /// The next event on this connection (for `send --wait`).
        pub async fn next_event(&mut self, timeout: Duration) -> Result<EventWait> {
            self.conn.next_event(timeout).await
        }

        /// Tells peekd this CLI has read the result of a `--wait` ask.
        pub async fn acknowledge_result(
            &mut self,
            ask_id: &silicon_peek_client::ids::AskId,
        ) -> Result<()> {
            self.conn.acknowledge_result(ask_id).await
        }
    }

    /// Connects to a running peekd without starting anything. `Ok(None)`
    /// when nothing answers the socket.
    pub async fn connect_existing(timeout: Duration) -> Result<Option<Service>> {
        let path = socket_path();
        let mut conn = match DaemonConnection::connect(&path, timeout).await {
            Ok(c) => c,
            Err(e) if *e.code() == ErrorCode::DaemonUnavailable => return Ok(None),
            Err(e) => return Err(e),
        };
        let hello = conn.hello(&hello_message(), REQUEST_TIMEOUT).await?;
        Ok(Some(Service { conn, hello }))
    }

    fn peekd_older(e: &Error) -> bool {
        match e.code() {
            ErrorCode::AppUpdatePending => true,
            // peekd could not intersect protocols; it is older when its
            // newest protocol is below every protocol this CLI speaks.
            ErrorCode::CliOutdated => e
                .details()
                .and_then(|d| d.get("peekd_protocols"))
                .and_then(Value::as_array)
                .and_then(|a| a.iter().filter_map(Value::as_u64).max())
                .is_some_and(|max| {
                    silicon_peek_client::ipc::SUPPORTED_PROTOCOLS
                        .iter()
                        .all(|p| u64::from(*p) > max)
                }),
            _ => false,
        }
    }

    /// `ensure_service()` (BLUEPRINT §1.8).
    pub async fn ensure_service() -> Result<Service> {
        match connect_existing(Duration::from_secs(2)).await {
            Ok(Some(s)) => return Ok(s),
            Ok(None) => {}
            Err(e) if peekd_older(&e) => return wait_for_update(e).await,
            Err(e) => return Err(e),
        }
        let paths = paths()?;
        ensure_app(&paths).await?;
        launch(&paths).await?;
        if let Some(s) = poll(POLL_BUDGET).await? {
            return Ok(s);
        }
        Err(not_started(&paths))
    }

    async fn poll(budget: Duration) -> Result<Option<Service>> {
        let deadline = Instant::now() + budget;
        loop {
            match connect_existing(Duration::from_millis(500)).await {
                Ok(Some(s)) => return Ok(Some(s)),
                Err(e)
                    if matches!(
                        e.code(),
                        ErrorCode::DaemonIdentityMismatch | ErrorCode::CliOutdated
                    ) =>
                {
                    return Err(e);
                }
                Ok(None) | Err(_) => {}
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    async fn wait_for_update(first: Error) -> Result<Service> {
        // hello already told peekd about the bundled build; peekd installs
        // it and restarts. Offer the zip on disk too, then wait up to 30 s.
        if let Ok(paths) = paths() {
            let _ = ensure_app(&paths).await;
        }
        let deadline = Instant::now() + UPDATE_WAIT;
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            match connect_existing(Duration::from_millis(500)).await {
                Ok(Some(s)) => return Ok(s),
                Ok(None) => {}
                Err(e) if peekd_older(&e) => {}
                Err(e) => return Err(e),
            }
            if Instant::now() >= deadline {
                return Err(Error::new(
                    ErrorCode::AppUpdatePending,
                    format!(
                        "peekd is older than this peek ({}) and did not update within 30 s: {}",
                        silicon_peek_client::VERSION,
                        first.message()
                    ),
                )
                .with_hint("the update waits until no bubble, recording or ask is on screen; retry in a minute, or run `peek app update`")
                .with_retryable(true));
            }
        }
    }

    fn not_started(paths: &Paths) -> Error {
        let log = last_line(&paths.peekd_log());
        let status = tail(&paths.install_status(), 5);
        let mut message = format!(
            "Peek.app did not bring up peekd within {} s (socket {})",
            POLL_BUDGET.as_secs(),
            socket_path().display()
        );
        if let Some(l) = &log {
            let _ = write!(message, "; last peekd.log line: {l}");
        }
        Error::new(ErrorCode::PeekServiceUnavailable, message)
            .with_hint(format!(
                "open {} once; logs: {} and {}",
                paths.app.display(),
                paths.peekd_log().display(),
                paths.install_status().display()
            ))
            .with_details(json!({
                "socket": socket_path().display().to_string(),
                "peekd_log_last": log,
                "install_status_tail": status,
            }))
    }

    /// The last non-empty line of a text file.
    pub fn last_line(path: &Path) -> Option<String> {
        tail(path, 1).pop()
    }

    /// The last `n` non-empty lines of a text file (reads at most 64 KiB).
    pub fn tail(path: &Path, n: usize) -> Vec<String> {
        let Ok(mut f) = File::open(path) else {
            return Vec::new();
        };
        let len = f.metadata().map_or(0, |m| m.len());
        let mut buf = Vec::new();
        if len > 65_536 {
            use std::io::Seek as _;
            let _ = f.seek(std::io::SeekFrom::Start(len - 65_536));
        }
        let _ = f.take(65_536).read_to_end(&mut buf);
        let text = String::from_utf8_lossy(&buf);
        let lines: Vec<String> = text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(str::to_owned)
            .collect();
        lines[lines.len().saturating_sub(n)..].to_vec()
    }

    fn log_file(paths: &Paths) -> Stdio {
        OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(paths.log())
            .map_or_else(|_| Stdio::null(), Stdio::from)
    }

    fn log_pair(paths: &Paths) -> (Stdio, Stdio) {
        let out = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(paths.log());
        match out.and_then(|f| f.try_clone().map(|g| (f, g))) {
            Ok((a, b)) => (Stdio::from(a), Stdio::from(b)),
            Err(_) => (Stdio::null(), Stdio::null()),
        }
    }

    /// Runs a short query and captures its stdout (stdin is /dev/null).
    async fn query(program: &str, args: &[&OsStr], timeout: Duration) -> Result<(bool, String)> {
        let mut command = tokio::process::Command::new(program);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let child = command.spawn().map_err(|e| {
            Error::new(
                ErrorCode::PeekServiceUnavailable,
                format!("could not run {program}: {e}"),
            )
        })?;
        let out = tokio::time::timeout(timeout, child.wait_with_output())
            .await
            .map_err(|_| {
                Error::new(
                    ErrorCode::PeekServiceUnavailable,
                    format!("{program} did not finish within {} s", timeout.as_secs()),
                )
            })?
            .map_err(|e| {
                Error::new(
                    ErrorCode::PeekServiceUnavailable,
                    format!("waiting for {program} failed: {e}"),
                )
            })?;
        let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            if !err.trim().is_empty() {
                text.push_str(err.trim());
            }
        }
        Ok((out.status.success(), text))
    }

    /// Runs a child whose output goes to `cli.log`; returns whether it succeeded.
    async fn run_logged(
        paths: &Paths,
        program: &str,
        args: &[&OsStr],
        timeout: Duration,
    ) -> Result<bool> {
        let (out, err) = log_pair(paths);
        let mut command = tokio::process::Command::new(program);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err)
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|e| {
            Error::new(
                ErrorCode::PeekServiceUnavailable,
                format!("could not run {program}: {e}"),
            )
        })?;
        let status = tokio::time::timeout(timeout, child.wait())
            .await
            .map_err(|_| {
                Error::new(
                    ErrorCode::PeekServiceUnavailable,
                    format!("{program} did not finish within {} s", timeout.as_secs()),
                )
            })?
            .map_err(|e| {
                Error::new(
                    ErrorCode::PeekServiceUnavailable,
                    format!("waiting for {program} failed: {e}"),
                )
            })?;
        Ok(status.success())
    }

    fn append_status(paths: &Paths, status: &str, step: &str, detail: &str) {
        if let Ok(mut f) = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(paths.install_status())
        {
            let _ = writeln!(f, "{status}\t{step}\t{detail}");
        }
    }

    async fn lock_install(paths: &Paths) -> Result<File> {
        let lock_path = paths.support.join("install.lock");
        let file = open_lock_file(&lock_path)?;
        let deadline = Instant::now() + INSTALL_LOCK_WAIT;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(file),
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(TryLockError::WouldBlock) => {
                    return Err(Error::new(
                        ErrorCode::PeekServiceUnavailable,
                        format!(
                            "{} stayed locked for 30 s (another peek or the installer is installing Peek.app)",
                            lock_path.display()
                        ),
                    )
                    .with_hint("retry in a minute"));
                }
                Err(TryLockError::Error(e)) => {
                    return Err(Error::new(
                        ErrorCode::PeekServiceUnavailable,
                        format!("locking {} failed: {e}", lock_path.display()),
                    ));
                }
            }
        }
    }

    fn offer_name(info: &AppOfferInfo) -> String {
        let sha12: String = info.zip_sha256.chars().take(12).collect();
        format!("{}-{sha12}.app", info.bundle_version)
    }

    fn offer(paths: &Paths, payload: &Payload) -> Result<bool> {
        let base = paths.offers().join(offer_name(&payload.info));
        let zip = base.with_extension("app.zip");
        let info = base.with_extension("app.info");
        if zip.is_file() && info.is_file() {
            return Ok(false);
        }
        let pid = std::process::id();
        let tmp_zip = paths
            .offers()
            .join(format!(".{}.zip.{pid}.tmp", offer_name(&payload.info)));
        let tmp_info = paths
            .offers()
            .join(format!(".{}.info.{pid}.tmp", offer_name(&payload.info)));
        let result = (|| -> std::io::Result<()> {
            std::fs::copy(&payload.zip, &tmp_zip)?;
            std::fs::copy(payload.pkg.join("Peek.app.info"), &tmp_info)?;
            std::fs::rename(&tmp_info, &info)?;
            std::fs::rename(&tmp_zip, &zip)
        })();
        if let Err(e) = result {
            let _ = std::fs::remove_file(&tmp_zip);
            let _ = std::fs::remove_file(&tmp_info);
            return Err(Error::new(
                ErrorCode::PeekServiceUnavailable,
                format!(
                    "offering build {} at {} failed: {e}",
                    payload.info.bundle_version,
                    zip.display()
                ),
            ));
        }
        Ok(true)
    }

    fn sha256_file(path: &Path) -> std::io::Result<String> {
        let mut f = File::open(path)?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 1 << 16];
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Ok(hex::encode(hasher.finalize()))
    }

    async fn plist_value(plist: &Path, key: &str) -> Option<String> {
        let (ok, out) = query(
            "/usr/bin/plutil",
            &[
                OsStr::new("-extract"),
                OsStr::new(key),
                OsStr::new("raw"),
                OsStr::new("-o"),
                OsStr::new("-"),
                plist.as_os_str(),
            ],
            QUERY_TIMEOUT,
        )
        .await
        .ok()?;
        let v = out.trim().to_owned();
        (ok && !v.is_empty()).then_some(v)
    }

    /// `(CFBundleVersion, CFBundleShortVersionString, CFBundleIdentifier)` of a bundle.
    pub async fn bundle_info(app: &Path) -> (Option<u64>, Option<String>, Option<String>) {
        let plist = app.join("Contents/Info.plist");
        if !plist.is_file() {
            return (None, None, None);
        }
        let build = plist_value(&plist, "CFBundleVersion")
            .await
            .and_then(|v| v.parse().ok());
        let short = plist_value(&plist, "CFBundleShortVersionString").await;
        let id = plist_value(&plist, "CFBundleIdentifier").await;
        (build, short, id)
    }

    /// Verifies a bundle's signature: the Developer ID requirement for
    /// release builds, plain strict verification for dev builds.
    pub async fn verify_signature(app: &Path, bundle_id: &str, team: &str) -> Result<bool> {
        let req = format!("-R={}", requirement(bundle_id, team));
        let mut args: Vec<&OsStr> = vec![
            OsStr::new("--verify"),
            OsStr::new("--deep"),
            OsStr::new("--strict"),
        ];
        if !team.is_empty() {
            args.push(OsStr::new(&req));
        }
        args.push(app.as_os_str());
        let (ok, _) = query("/usr/bin/codesign", &args, CHILD_TIMEOUT).await?;
        Ok(ok)
    }

    async fn macos_major() -> Option<u64> {
        let (ok, out) = query(
            "/usr/bin/sw_vers",
            &[OsStr::new("-productVersion")],
            QUERY_TIMEOUT,
        )
        .await
        .ok()?;
        if !ok {
            return None;
        }
        out.trim().split('.').next()?.parse().ok()
    }

    /// The outcome of `ensure_app()`.
    #[derive(Clone, Debug, Serialize, Deserialize)]
    pub struct AppInstall {
        /// `~/Applications/Peek.app`.
        pub path: String,
        /// Whether this call installed it.
        pub installed_now: bool,
        /// The bundled build offered to peekd, if any.
        pub offered_build: Option<u64>,
    }

    /// `ensure_app()` (BLUEPRINT §1.8): offer the bundled build and install
    /// the first Peek.app. Never modifies an existing bundle.
    pub async fn ensure_app(paths: &Paths) -> Result<AppInstall> {
        ensure_private_dir(&paths.support)?;
        ensure_private_dir(&paths.offers())?;
        let _lock = lock_install(paths).await?;
        let payload = bundled_payload();
        append_status(
            paths,
            "run",
            &Timestamp::now().to_rfc3339(),
            &format!(
                "peek {} ensure_app from {}",
                silicon_peek_client::VERSION,
                payload.as_ref().map_or_else(
                    || "a build without Peek.app.zip".to_owned(),
                    |p| p.pkg.display().to_string()
                )
            ),
        );
        let offered_build = match &payload {
            Some(p) => {
                match offer(paths, p) {
                    Ok(true) => append_status(
                        paths,
                        "ok",
                        "offer",
                        &format!("offered build {}", p.info.bundle_version),
                    ),
                    Ok(false) => append_status(
                        paths,
                        "ok",
                        "offer",
                        &format!("build {} is already offered", p.info.bundle_version),
                    ),
                    Err(e) => append_status(paths, "degraded", "offer", e.message()),
                }
                Some(p.info.bundle_version)
            }
            None => None,
        };
        let path = paths.app.display().to_string();
        if std::fs::symlink_metadata(&paths.app).is_ok() {
            append_status(
                paths,
                "skip",
                "install",
                &format!("Peek.app is already at {path}; it applies newer offers itself"),
            );
            return Ok(AppInstall {
                path,
                installed_now: false,
                offered_build,
            });
        }
        let payload = payload.ok_or_else(|| {
            let exe = std::env::current_exe()
                .map_or_else(|_| "this peek".to_owned(), |p| p.display().to_string());
            Error::new(
                ErrorCode::PeekServiceUnavailable,
                format!("Peek.app is not installed at {path}, and {exe} has no Peek.app.zip in its package directory to install it from"),
            )
            .with_hint("install peek through Honeycomb (honeycomb install 'peek'), then run `peek app install` from that copy")
        })?;
        install(paths, &payload).await?;
        Ok(AppInstall {
            path,
            installed_now: true,
            offered_build,
        })
    }

    async fn install(paths: &Paths, payload: &Payload) -> Result<()> {
        let info = &payload.info;
        let fail = |step: &str, msg: String| {
            append_status(paths, "degraded", step, &msg);
            Error::new(ErrorCode::PeekServiceUnavailable, msg)
        };
        if let (Some(have), Ok(need)) = (
            macos_major().await,
            info.minimum_system_version
                .split('.')
                .next()
                .unwrap_or_default()
                .parse::<u64>(),
        ) && have < need
        {
            return Err(Error::new(
                ErrorCode::PlatformUnsupported,
                format!(
                    "this Mac runs macOS {have}, older than Peek's minimum {}",
                    info.minimum_system_version
                ),
            )
            .with_hint(
                "update macOS; the peek CLI still handles iam, login and config on this Mac",
            ));
        }
        let actual = sha256_file(&payload.zip).map_err(|e| {
            fail(
                "verify",
                format!("reading {} failed: {e}", payload.zip.display()),
            )
        })?;
        if !actual.eq_ignore_ascii_case(&info.zip_sha256) {
            return Err(fail(
                "verify",
                format!(
                    "{} has SHA-256 {actual}, but Peek.app.info says {}; the package is damaged",
                    payload.zip.display(),
                    info.zip_sha256
                ),
            )
            .with_hint("reinstall peek: honeycomb install 'peek'"));
        }
        std::fs::create_dir_all(&paths.apps).map_err(|e| {
            fail(
                "install",
                format!("creating {} failed: {e}", paths.apps.display()),
            )
        })?;
        let stage = paths
            .apps
            .join(format!(".Peek.app.install.{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&stage);
        let result = install_staged(paths, payload, &stage).await;
        let _ = std::fs::remove_dir_all(&stage);
        result.inspect_err(|e| append_status(paths, "degraded", "install", e.message()))
    }

    async fn install_staged(paths: &Paths, payload: &Payload, stage: &Path) -> Result<()> {
        let info = &payload.info;
        let err = |msg: String| Error::new(ErrorCode::PeekServiceUnavailable, msg);
        // Plain `ditto -x -k`: never --norsrc, which can write AppleDouble
        // files into the bundle and break its signature.
        let unpacked = run_logged(
            paths,
            "/usr/bin/ditto",
            &[
                OsStr::new("-x"),
                OsStr::new("-k"),
                payload.zip.as_os_str(),
                stage.as_os_str(),
            ],
            CHILD_TIMEOUT,
        )
        .await?;
        if !unpacked {
            return Err(err(format!(
                "ditto could not unpack {}",
                payload.zip.display()
            )));
        }
        let staged_app = stage.join("Peek.app");
        let expected = if info.team_id.is_empty() {
            DEV_BUNDLE_ID
        } else {
            BUNDLE_ID
        };
        let (_, _, got_id) = bundle_info(&staged_app).await;
        if info.bundle_id != expected || got_id.as_deref() != Some(expected) {
            return Err(err(format!(
                "the unpacked bundle has identifier {:?} and Peek.app.info says {:?}; expected {expected}",
                got_id, info.bundle_id
            )));
        }
        let team = info.team_id.as_str();
        if team.is_empty() {
            append_status(
                paths,
                "ok",
                "verify",
                "dev build (no team id): verifying the signature only",
            );
        } else if team != TEAM_ID {
            return Err(err(format!(
                "Peek.app.info names team {team}; peek installs only builds signed by {TEAM_ID}"
            )));
        }
        if !verify_signature(&staged_app, expected, team).await? {
            return Err(err(format!(
                "the unpacked Peek.app fails codesign --verify --deep --strict for {expected}{}",
                if team.is_empty() { String::new() } else { format!(" (Developer ID, team {team})") }
            ))
            .with_hint("the package may be damaged or tampered with; reinstall peek: honeycomb install 'peek'"));
        }
        // Quarantine would stop a hidden launch at the first-open dialog.
        let _ = run_logged(
            paths,
            "/usr/bin/xattr",
            &[
                OsStr::new("-dr"),
                OsStr::new("com.apple.quarantine"),
                staged_app.as_os_str(),
            ],
            CHILD_TIMEOUT,
        )
        .await;
        match crate::sys::rename_exclusive(&staged_app, &paths.app) {
            Ok(()) => {
                append_status(
                    paths,
                    "ok",
                    "install",
                    &format!(
                        "installed Peek.app {} (build {}) at {}",
                        info.short_version,
                        info.bundle_version,
                        paths.app.display()
                    ),
                );
                Ok(())
            }
            Err(e) if e.raw_os_error() == Some(libc::EEXIST) => {
                append_status(
                    paths,
                    "skip",
                    "install",
                    "another installer placed Peek.app first",
                );
                Ok(())
            }
            Err(e) => Err(err(format!(
                "moving the verified Peek.app to {} failed: {e}",
                paths.app.display()
            ))),
        }
    }

    fn gui_target() -> String {
        format!("gui/{}/{AGENT_LABEL}", sys::uid())
    }

    /// Whether launchd has peekd's agent loaded (`None` when launch hooks
    /// forbid talking to launchd).
    pub async fn agent_loaded() -> Option<bool> {
        if no_launch() {
            return None;
        }
        let target = gui_target();
        query(
            "/bin/launchctl",
            &[OsStr::new("print"), OsStr::new(&target)],
            QUERY_TIMEOUT,
        )
        .await
        .ok()
        .map(|(ok, _)| ok)
    }

    async fn aqua() -> Result<(bool, String)> {
        let (_, out) = query(
            "/bin/launchctl",
            &[OsStr::new("managername")],
            QUERY_TIMEOUT,
        )
        .await?;
        let name = out.trim().to_owned();
        Ok((name == "Aqua", name))
    }

    fn launch_disabled() -> Error {
        Error::new(
            ErrorCode::PeekServiceUnavailable,
            "starting Peek.app is disabled for this process (PEEK_INSTALL_NO_LAUNCH=1 or PEEK_NO_SERVICES=1)",
        )
        .with_hint("unset PEEK_INSTALL_NO_LAUNCH / PEEK_NO_SERVICES, or start peekd (or ~/Applications/Peek.app) yourself")
    }

    fn no_gui(manager: &str) -> Error {
        Error::new(
            ErrorCode::NoGuiSession,
            format!(
                "this process has no GUI session (launchctl managername: {manager}), so Peek.app cannot start"
            ),
        )
        .with_hint("run the command from the logged-in user's session (for example Terminal on the Mac), or open ~/Applications/Peek.app once")
    }

    /// Starts peekd: kickstart the agent, else open the app in the background.
    pub async fn launch(paths: &Paths) -> Result<&'static str> {
        if no_launch() {
            return Err(launch_disabled());
        }
        let target = gui_target();
        if agent_loaded().await == Some(true) {
            run_logged(
                paths,
                "/bin/launchctl",
                &[OsStr::new("kickstart"), OsStr::new(&target)],
                QUERY_TIMEOUT,
            )
            .await?;
            return Ok("launchctl");
        }
        let (is_aqua, manager) = aqua().await?;
        if !is_aqua {
            return Err(no_gui(&manager));
        }
        let opened = run_logged(
            paths,
            "/usr/bin/open",
            &[
                OsStr::new("-g"),
                OsStr::new("-j"),
                paths.app.as_os_str(),
                OsStr::new("--args"),
                OsStr::new("--launched-by"),
                OsStr::new("cli"),
            ],
            Duration::from_secs(15),
        )
        .await?;
        if !opened {
            return Err(Error::new(
                ErrorCode::PeekServiceUnavailable,
                format!(
                    "open -g -j {} failed; see {}",
                    paths.app.display(),
                    paths.log().display()
                ),
            ));
        }
        Ok("open")
    }

    /// `peek daemon restart`: `launchctl kickstart -k`, or start the app.
    pub async fn restart() -> Result<(Service, &'static str)> {
        if no_launch() {
            return Err(launch_disabled());
        }
        let paths = paths()?;
        let target = gui_target();
        let via = if agent_loaded().await == Some(true) {
            let ok = run_logged(
                &paths,
                "/bin/launchctl",
                &[
                    OsStr::new("kickstart"),
                    OsStr::new("-k"),
                    OsStr::new(&target),
                ],
                Duration::from_secs(20),
            )
            .await?;
            if !ok {
                return Err(Error::new(
                    ErrorCode::PeekServiceUnavailable,
                    format!(
                        "launchctl kickstart -k {target} failed; see {}",
                        paths.log().display()
                    ),
                ));
            }
            "launchctl"
        } else {
            ensure_app(&paths).await?;
            launch(&paths).await?
        };
        match poll(POLL_BUDGET).await? {
            Some(s) => Ok((s, via)),
            None => Err(not_started(&paths)),
        }
    }

    /// The newest build among the offers and the bundled payload.
    pub fn best_offer(paths: &Paths) -> Option<(u64, PathBuf, AppOfferInfo)> {
        let mut best: Option<(u64, PathBuf, AppOfferInfo)> =
            bundled_payload().map(|p| (p.info.bundle_version, p.zip.clone(), p.info));
        if let Ok(entries) = std::fs::read_dir(paths.offers()) {
            for entry in entries.flatten() {
                let path = entry.path();
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if !name.ends_with(".app.info") || name.starts_with('.') {
                    continue;
                }
                let zip = path.with_file_name(name.replace(".app.info", ".app.zip"));
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let Ok(info) = AppOfferInfo::parse_sidecar(&text) else {
                    continue;
                };
                if zip.is_file()
                    && matches!(info.bundle_id.as_str(), BUNDLE_ID | DEV_BUNDLE_ID)
                    && best
                        .as_ref()
                        .is_none_or(|(b, _, _)| info.bundle_version > *b)
                {
                    best = Some((info.bundle_version, zip, info));
                }
            }
        }
        best
    }

    /// `peek app status` on macOS.
    pub async fn app_status() -> Result<Value> {
        let paths = paths()?;
        let installed = std::fs::symlink_metadata(&paths.app).is_ok();
        let (build, short, id) = bundle_info(&paths.app).await;
        let signature = match (&id, installed) {
            (Some(id), true) => {
                let team = if id == DEV_BUNDLE_ID { "" } else { TEAM_ID };
                match verify_signature(&paths.app, id, team).await {
                    Ok(true) => "valid",
                    Ok(false) => "invalid",
                    Err(_) => "unchecked",
                }
            }
            (None, true) => "unreadable",
            (_, false) => "absent",
        };
        let daemon = match connect_existing(Duration::from_secs(1)).await {
            Ok(Some(mut s)) => s
                .call(
                    &silicon_peek_client::ipc::cli::DaemonStatus {},
                    None,
                    Vec::new(),
                    REQUEST_TIMEOUT,
                )
                .await
                .ok()
                .map(|(r, _)| r),
            _ => None,
        };
        let agent = match agent_loaded().await {
            Some(true) => "loaded",
            Some(false) => "not_loaded",
            None => "unknown",
        };
        let bundled = bundled_payload().map(|p| {
            json!({"build": p.info.bundle_version, "short_version": p.info.short_version,
                   "zip_path": p.zip.display().to_string(), "team_id": p.info.team_id})
        });
        Ok(json!({
            "supported": true,
            "platform": platform(),
            "installed": installed,
            "path": paths.app.display().to_string(),
            "build": build,
            "short_version": short,
            "bundle_id": id,
            "signature": signature,
            "running": {
                "ui": daemon.as_ref().is_some_and(|d| d.ui.running),
                "daemon": daemon.is_some(),
            },
            "daemon": daemon,
            "agent": agent,
            "best_offer": best_offer(&paths).map(|(b, _, _)| b),
            "bundled": bundled,
            "last_install_script": tail(&paths.install_status(), 10),
        }))
    }

    /// Asks a running peekd (and through it the UI) to uninstall, falling
    /// back to `open … --args --uninstall`; waits for the bundle to go.
    pub async fn uninstall(auth: Option<&AuthBlock>) -> Result<Value> {
        let paths = paths()?;
        let path = paths.app.display().to_string();
        if std::fs::symlink_metadata(&paths.app).is_err() {
            return Ok(
                json!({"uninstalled": true, "was_installed": false, "path": path, "via": null}),
            );
        }
        let mut via = None;
        if let Ok(Some(mut s)) = connect_existing(Duration::from_secs(1)).await
            && s.call(&AppUninstall {}, auth, Vec::new(), REQUEST_TIMEOUT)
                .await
                .is_ok()
        {
            via = Some("peekd");
        }
        if via.is_none() {
            if no_launch() {
                return Err(launch_disabled());
            }
            let (is_aqua, manager) = aqua().await?;
            if !is_aqua {
                return Err(no_gui(&manager));
            }
            run_logged(
                &paths,
                "/usr/bin/open",
                &[
                    OsStr::new("-g"),
                    OsStr::new("-j"),
                    paths.app.as_os_str(),
                    OsStr::new("--args"),
                    OsStr::new("--uninstall"),
                ],
                Duration::from_secs(15),
            )
            .await?;
            via = Some("open");
        }
        let deadline = Instant::now() + UNINSTALL_WAIT;
        while std::fs::symlink_metadata(&paths.app).is_ok() {
            if Instant::now() >= deadline {
                return Err(Error::new(
                    ErrorCode::PeekServiceUnavailable,
                    format!("Peek.app did not remove itself from {path} within 15 s"),
                )
                .with_hint("quit Peek from its menu, then move ~/Applications/Peek.app to the Trash; its login item and helper go with it"));
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        Ok(json!({"uninstalled": true, "was_installed": true, "path": path, "via": via}))
    }

    /// `app.uninstall` and `doctor` (CLI-local IPC ops, shared with peekd).
    pub use silicon_peek_client::ipc::cli::{AppUninstall, Doctor as DoctorOp};

    /// Starts `peek __after-login` detached: `ensure_app` + launch + attach,
    /// after `peek login` printed its result (BLUEPRINT §2.4 step 6).
    pub fn spawn_after_login(store: &Path, api: &ApiUrl, context: Context) -> Result<()> {
        let exe = std::env::current_exe()
            .map_err(|e| Error::internal(format!("locating this executable failed: {e}")))?;
        let paths = paths()?;
        let _ = ensure_private_dir(&paths.support);
        let mut command = std::process::Command::new(exe);
        command
            .arg("__after-login")
            .arg("--store")
            .arg(store)
            .arg("--api-url")
            .arg(api.as_str())
            .arg("--context")
            .arg(context.to_string())
            .stdin(Stdio::null())
            .stdout(log_file(&paths))
            .stderr(log_file(&paths))
            .env_remove("PEEK_TEST_APP_SECRET");
        crate::sys::detach(&mut command);
        command.spawn().map(drop).map_err(|e| {
            Error::internal(format!(
                "starting the background Peek.app setup failed: {e}"
            ))
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn requirement_names_team_and_bundle() {
            let r = requirement(BUNDLE_ID, TEAM_ID);
            assert!(r.starts_with("anchor apple generic and identifier \"ai.tos.peek\""));
            assert!(r.ends_with("certificate leaf[subject.OU] = \"LTBSK59BJ2\""));
        }

        #[test]
        fn tails_skip_blank_lines() -> std::io::Result<()> {
            let t = tempfile::tempdir()?;
            let p = t.path().join("log");
            std::fs::write(&p, "a\n\nb\nc\n")?;
            assert_eq!(tail(&p, 2), vec!["b".to_owned(), "c".to_owned()]);
            assert_eq!(last_line(&p).as_deref(), Some("c"));
            assert!(tail(&t.path().join("missing"), 3).is_empty());
            Ok(())
        }

        #[test]
        fn offer_names() -> Result<()> {
            let info = AppOfferInfo::parse_sidecar(&format!(
                "bundle_id=ai.tos.peek\nbundle_version=1000\nshort_version=0.1.0\nteam_id=LTBSK59BJ2\nzip_sha256={}\nminimum_system_version=26.0\n",
                "ab".repeat(32)
            ))?;
            assert_eq!(offer_name(&info), "1000-abababababab.app");
            Ok(())
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod other {
    //! Stand-ins on platforms without Peek.app: every Mac-bound command checks
    //! [`super::require_mac`] first, so only the best-effort callers (login's
    //! attach, telemetry, `login status` daemon info) reach these. The
    //! signatures mirror the macOS implementations, hence the allows.
    #![allow(
        clippy::unused_async,
        clippy::unused_async_trait_impl,
        clippy::unnecessary_wraps
    )]

    use std::time::Duration;

    use silicon_peek_client::{
        Result,
        identity::{ApiUrl, Context},
        ipc::{AuthBlock, Op, cli::HelloResult},
        runtime::daemon::EventWait,
    };

    /// No peekd exists here; the type exists for API parity with macOS and
    /// is never constructed ([`connect_existing`] always answers `None`).
    #[derive(Debug)]
    #[allow(dead_code)]
    pub struct Service {
        /// peekd's answer to `hello` (never present here).
        pub hello: HelloResult,
    }

    impl Service {
        /// Unreachable: no [`Service`] is ever constructed on this platform.
        pub async fn call<O: Op>(
            &mut self,
            _op: &O,
            _auth: Option<&AuthBlock>,
            _blobs: Vec<Vec<u8>>,
            _timeout: Duration,
        ) -> Result<(O::Output, Vec<Vec<u8>>)> {
            Err(super::platform_unsupported(
                O::NAME,
                &silicon_peek_client::platform(),
            ))
        }

        /// Unreachable: no [`Service`] is ever constructed on this platform.
        pub async fn next_event(&mut self, _timeout: Duration) -> Result<EventWait> {
            Ok(EventWait::Closed)
        }

        /// Unreachable: no [`Service`] is ever constructed on this platform.
        pub async fn acknowledge_result(
            &mut self,
            _ask_id: &silicon_peek_client::ids::AskId,
        ) -> Result<()> {
            Ok(())
        }
    }

    /// There is never a running peekd here.
    pub async fn connect_existing(_timeout: Duration) -> Result<Option<Service>> {
        Ok(None)
    }

    /// Mac-bound: always `platform_unsupported`.
    pub async fn ensure_service() -> Result<Service> {
        Err(super::platform_unsupported(
            "status",
            &silicon_peek_client::platform(),
        ))
    }

    /// Nothing to install here.
    pub fn spawn_after_login(
        _store: &std::path::Path,
        _api: &ApiUrl,
        _context: Context,
    ) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn platform_unsupported_is_the_blueprint_json() {
        let e = platform_unsupported("send", "linux-x86_64");
        assert_eq!(
            e.envelope(),
            json!({"error":{"code":"platform_unsupported",
                "message":"peek send shows a bubble on a Mac through Peek.app; this peek runs on linux-x86_64.",
                "hint":"Run this Silicon on macOS 26+ with Peek.app installed, or use dm for text conversations.",
                "retryable":false,"request_id":null,
                "details":{"platform":"linux-x86_64","supported_platforms":["macos-aarch64","macos-x86_64"],
                           "docs_url":"https://peek.teamofsilicons.com/docs/platforms"}}})
        );
        assert_eq!(e.exit_code().code(), 4);
        for c in [
            "register side",
            "register drawing",
            "unregister",
            "ask get",
            "history",
            "status",
            "app install",
            "daemon status",
        ] {
            let m = platform_unsupported(c, "windows-x86_64")
                .message()
                .to_owned();
            assert!(m.starts_with(&format!("peek {c} ")), "{m}");
            assert!(m.ends_with("; this peek runs on windows-x86_64."), "{m}");
        }
    }

    #[test]
    fn require_mac_matches_the_build() {
        assert_eq!(require_mac("send").is_ok(), cfg!(target_os = "macos"));
    }
}
