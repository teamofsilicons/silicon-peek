//! peekd's configuration: where things live, which UI executable may
//! connect, which commands run, and every timing, so tests can run a whole
//! daemon in a temp directory with millisecond schedules.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use silicon_peek_client::{
    Error, ErrorCode, Result,
    identity::ApiUrl,
    ipc::cli::AppOfferInfo,
    runtime::{daemon, sys},
    telemetry,
};

use crate::commands::{CommandRunner, SystemCommands};

/// The production bundle identifier of Peek.app (D23).
pub const BUNDLE_ID: &str = "ai.tos.peek";
/// The development bundle identifier (D23).
pub const DEV_BUNDLE_ID: &str = "ai.tos.peek.dev";
/// The Developer ID team (D23).
pub const TEAM_ID: &str = "LTBSK59BJ2";

/// Test-only override of the accepted UI executable (exact path).
pub const UI_EXECUTABLE_ENV: &str = "PEEK_UI_EXECUTABLE";
/// Override of `~/Library/Application Support/Peek` (isolated runs and
/// tests; Peek.app honours the same variable).
pub const SUPPORT_DIR_ENV: &str = "PEEK_SUPPORT_DIR";
/// Override of `~/Library/Caches/Peek` (isolated runs; Peek.app honours the
/// same variable). peekd keeps all of its own caches under the support
/// directory (§1.7) and writes nothing here.
pub const CACHES_DIR_ENV: &str = "PEEK_CACHES_DIR";
/// `1`: an isolated run. peekd never launches Peek.app and never runs the
/// updater or the CLI watchdog (no `open`, `launchctl`, `ditto`, `codesign`,
/// `honeycomb` for the real user). Peek.app honours it too (no `SMAppService`,
/// no login items, never spawns peekd).
pub const NO_SERVICES_ENV: &str = "PEEK_NO_SERVICES";
/// Test-only override of `~/Applications`.
pub const APPLICATIONS_DIR_ENV: &str = "PEEK_APPLICATIONS_DIR";
/// The install hooks `scripts/install-app.sh` and the CLI's `ensure_app`
/// read (BLUEPRINT §4.3). peekd honours them too, resolving exactly as the
/// CLI does, so a test that sets them sees one Applications and one support
/// directory: `PEEK_INSTALL_SUPPORT_DIR` wins over `PEEK_SUPPORT_DIR`,
/// `PEEK_APPLICATIONS_DIR` over `PEEK_INSTALL_APPLICATIONS_DIR`, and
/// `PEEK_INSTALL_NO_LAUNCH=1` means the same as `PEEK_NO_SERVICES=1`.
pub const INSTALL_HOOK_ENVS: [&str; 3] = [
    "PEEK_INSTALL_SUPPORT_DIR",
    "PEEK_INSTALL_APPLICATIONS_DIR",
    "PEEK_INSTALL_NO_LAUNCH",
];
/// Where peekd relays telemetry (defaults to production).
pub const API_URL_ENV: &str = "PEEK_API_URL";

/// Which executable may connect with `role: "ui"` (§1.5: the peer PID's
/// executable must be Peek.app's).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UiExecutableRule {
    /// Exactly this (canonical) path: the sibling `Contents/MacOS/Peek` of the
    /// bundle peekd runs from, or a test override.
    Exact(PathBuf),
    /// Any path ending in `/Peek.app/Contents/MacOS/Peek` (peekd running
    /// outside a bundle, i.e. a development build).
    PeekAppSuffix,
}

impl UiExecutableRule {
    /// The rule for this process: `PEEK_UI_EXECUTABLE` when set, else the
    /// sibling of our own bundle, else the suffix rule.
    #[must_use]
    pub fn detect() -> Self {
        if let Some(p) = std::env::var_os(UI_EXECUTABLE_ENV).filter(|p| !p.is_empty()) {
            let p = PathBuf::from(p);
            return Self::Exact(p.canonicalize().unwrap_or(p));
        }
        let exe = std::env::current_exe().and_then(|p| p.canonicalize()).ok();
        if let Some(contents) = exe
            .as_deref()
            .and_then(Path::parent)
            .filter(|helpers| helpers.file_name().is_some_and(|n| n == "Helpers"))
            .and_then(Path::parent)
            .filter(|c| c.file_name().is_some_and(|n| n == "Contents"))
        {
            return Self::Exact(contents.join("MacOS").join("Peek"));
        }
        Self::PeekAppSuffix
    }

    /// Whether `path` satisfies the rule.
    #[must_use]
    pub fn accepts(&self, path: &Path) -> bool {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        match self {
            Self::Exact(expected) => canonical == *expected || path == expected,
            Self::PeekAppSuffix => canonical.ends_with("Peek.app/Contents/MacOS/Peek"),
        }
    }

    /// A description for error messages.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Exact(p) => p.display().to_string(),
            Self::PeekAppSuffix => "…/Peek.app/Contents/MacOS/Peek".to_owned(),
        }
    }
}

/// Where Honeycomb is found, in Stemcell's lookup order (gap-honeycomb §1.1).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HoneycombLocator {
    /// `$SILICON_HONEYCOMB`, taken as-is.
    pub env_override: Option<PathBuf>,
    /// `$PATH` of peekd's environment.
    pub path_var: Option<OsString>,
    /// Silicon's managed prefix (default `~/.local/share/silicon`).
    pub managed_prefix: Option<PathBuf>,
    /// The account's real home.
    pub real_home: PathBuf,
}

impl HoneycombLocator {
    /// From the process environment.
    #[must_use]
    pub fn from_env(real_home: &Path) -> Self {
        Self {
            env_override: std::env::var_os("SILICON_HONEYCOMB")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
            path_var: std::env::var_os("PATH"),
            managed_prefix: Some(real_home.join(".local/share/silicon")),
            real_home: real_home.to_path_buf(),
        }
    }

    /// The Honeycomb binary for a registry whose home is `registry_home`.
    #[must_use]
    pub fn resolve(&self, registry_home: &Path) -> Option<PathBuf> {
        if let Some(p) = &self.env_override {
            return Some(p.clone());
        }
        if let Some(path) = &self.path_var {
            for dir in std::env::split_paths(path) {
                let candidate = dir.join("honeycomb");
                if is_executable(&candidate) {
                    return Some(candidate);
                }
            }
        }
        let rel = Path::new(".honeycomb/dir/system/bin/honeycomb");
        let mut candidates = Vec::new();
        if let Some(prefix) = &self.managed_prefix {
            candidates.push(prefix.join(rel));
        }
        candidates.push(registry_home.join(rel));
        candidates.push(self.real_home.join(rel));
        candidates.into_iter().find(|c| is_executable(c))
    }
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// Every schedule and timeout peekd uses. [`Timings::default`] is
/// production; tests shrink them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Timings {
    /// Outbox backoff after a transient failure (§3.6): 1 s, 2 s, 5 s, 15 s,
    /// 30 s, 1 m, 2 m, 5 m, 10 m …
    pub outbox_backoff: Vec<Duration>,
    /// … then every 15 minutes until the row's max age.
    pub outbox_steady: Duration,
    /// Retry interval while a Ting type is missing (§3.6: every 15 min).
    pub ting_type_missing_retry: Duration,
    /// Longest the outbox worker sleeps between scans.
    pub outbox_idle: Duration,
    /// Rows stuck in `authority_required` are retried this often.
    pub authority_sweep: Duration,
    /// TTS retry delays before any audio (§1.9.3: 250 ms → 1 s, + jitter).
    pub tts_retry: Vec<Duration>,
    /// Budget to the first TTS audio byte (§1.9.3: 5 s).
    pub tts_first_audio_budget: Duration,
    /// Longest silence inside a TTS body before it is abandoned.
    pub tts_idle: Duration,
    /// STT retry delays (§1.9.5: 250 ms → 1 s).
    pub stt_retry: Vec<Duration>,
    /// Whole STT budget (notes/speech §5.3: about 20 s).
    pub stt_budget: Duration,
    /// Refresh retry delays for interactive paths (speech, drawings).
    pub refresh_retry: Vec<Duration>,
    /// How long peekd waits for Peek.app to connect when it needs it.
    pub ui_connect_wait: Duration,
    /// Timeout of ordinary peekd→UI requests.
    pub ui_request_timeout: Duration,
    /// Timeout of `drawing.validate`.
    pub drawing_validate_timeout: Duration,
    /// Telemetry relay period (§6.3: 10 s).
    pub telemetry_interval: Duration,
    /// Update check period (§4.6: hourly).
    pub update_interval: Duration,
    /// Delay before the first update check after start.
    pub update_initial_delay: Duration,
    /// How often a deferred update asks the UI again.
    pub update_defer_poll: Duration,
    /// Longest an update is deferred in one attempt (§4.6: 6 h).
    pub update_defer_max: Duration,
    /// How long Peek.app gets to quit for an update before SIGTERM (10 s).
    pub ui_quit_wait: Duration,
    /// How long a CLI copy may lag the newest release before the watchdog
    /// updates it (§4.6: 2 h).
    pub watchdog_behind: Duration,
    /// Grace before launching Peek.app at start when work is pending.
    pub launch_grace: Duration,
    /// Extra time a non-ask bubble may stay before peekd closes it itself.
    pub bubble_grace: Duration,
    /// How long peekd waits for a `--wait` connection to take an answer.
    pub waiter_ack: Duration,
    /// Cooldown between pre-warms of one Silicon's session.
    pub prewarm_cooldown: Duration,
}

impl Default for Timings {
    fn default() -> Self {
        let s = Duration::from_secs;
        let ms = Duration::from_millis;
        Self {
            outbox_backoff: vec![
                s(1),
                s(2),
                s(5),
                s(15),
                s(30),
                s(60),
                s(120),
                s(300),
                s(600),
            ],
            outbox_steady: s(900),
            ting_type_missing_retry: s(900),
            outbox_idle: s(30),
            authority_sweep: s(600),
            tts_retry: vec![ms(250), s(1)],
            tts_first_audio_budget: s(5),
            tts_idle: s(15),
            stt_retry: vec![ms(250), s(1)],
            stt_budget: s(20),
            refresh_retry: vec![ms(500), s(2)],
            ui_connect_wait: s(20),
            ui_request_timeout: s(10),
            drawing_validate_timeout: s(90),
            telemetry_interval: s(10),
            update_interval: s(3600),
            update_initial_delay: s(60),
            update_defer_poll: s(300),
            update_defer_max: s(6 * 3600),
            ui_quit_wait: s(10),
            watchdog_behind: s(2 * 3600),
            launch_grace: s(3),
            bubble_grace: s(30),
            waiter_ack: s(2),
            prewarm_cooldown: s(60),
        }
    }
}

/// Everything peekd needs to run.
#[derive(Clone, Debug)]
#[allow(clippy::struct_excessive_bools)] // one switch per subsystem (UI launch, telemetry, log file, updater)
pub struct DaemonConfig {
    /// The socket (`/var/tmp/silicon-peek-<uid>/peekd.sock`).
    pub socket_path: PathBuf,
    /// The single-instance lock (`<socket dir>/peekd.lock`).
    pub lock_path: PathBuf,
    /// `~/Library/Application Support/Peek`.
    pub support_dir: PathBuf,
    /// `~/Library/Caches/Peek` (Peek.app's; peekd writes nothing there).
    pub caches_dir: PathBuf,
    /// The account's real home (getpwuid), never `SILICON_HOME`.
    pub real_home: PathBuf,
    /// `~/Applications` (where Peek.app lives, D12).
    pub applications_dir: PathBuf,
    /// Which executable may connect as the UI.
    pub ui_executable: UiExecutableRule,
    /// Whether peekd may launch Peek.app (`open -g -j`); false when the UI
    /// spawned peekd itself (`--parent-ui`).
    pub launch_ui: bool,
    /// The command runner.
    pub commands: Arc<dyn CommandRunner>,
    /// Where telemetry is relayed (`POST /api/web/telemetry`).
    pub telemetry_api: ApiUrl,
    /// Process-level telemetry switch (env opt-out wins).
    pub telemetry_enabled: bool,
    /// Every schedule.
    pub timings: Timings,
    /// Peek.app's bundle identifier for this channel.
    pub bundle_id: String,
    /// This build's `CFBundleVersion`.
    pub own_build: u64,
    /// Honeycomb lookup for the CLI watchdog.
    pub honeycomb: HoneycombLocator,
    /// Write `peekd.log` (off in tests).
    pub log_to_file: bool,
    /// Run the hourly update check and CLI watchdog.
    pub updates_enabled: bool,
}

impl DaemonConfig {
    /// The production configuration from this process's environment.
    ///
    /// # Errors
    /// `invalid_silicon_home` when the account has no home directory, or
    /// `invalid_input` for a malformed `PEEK_API_URL`.
    pub fn from_env(launch_ui: bool) -> Result<Self> {
        let real_home = sys::real_home()?;
        let socket_path = daemon::socket_path();
        let lock_path = socket_path
            .parent()
            .map_or_else(daemon::lock_path, |d| d.join("peekd.lock"));
        let dirs = IsolatedDirs::resolve(&real_home, |k| std::env::var_os(k));
        let (support_dir, caches_dir, applications_dir, isolated) =
            (dirs.support, dirs.caches, dirs.applications, dirs.isolated);
        let telemetry_api = match std::env::var(API_URL_ENV) {
            Ok(v) if !v.is_empty() => ApiUrl::parse(&v)?,
            _ => ApiUrl::production(),
        };
        Ok(Self {
            socket_path,
            lock_path,
            support_dir,
            caches_dir,
            applications_dir,
            ui_executable: UiExecutableRule::detect(),
            launch_ui: launch_ui && !isolated,
            commands: Arc::new(SystemCommands),
            telemetry_api,
            telemetry_enabled: !telemetry::env_opt_out(),
            timings: Timings::default(),
            bundle_id: BUNDLE_ID.to_owned(),
            own_build: own_build()?,
            honeycomb: HoneycombLocator::from_env(&real_home),
            real_home,
            log_to_file: true,
            updates_enabled: !isolated,
        })
    }

    /// A configuration rooted in `root` (tests and development): socket,
    /// lock, support directory, applications directory and home all live
    /// under it, the UI is never launched and updates are off.
    ///
    /// # Errors
    /// `internal_error` if `root` cannot be prepared.
    pub fn rooted(root: &Path) -> Result<Self> {
        let mk = |p: PathBuf| -> Result<PathBuf> {
            std::fs::create_dir_all(&p)
                .map_err(|e| Error::internal(format!("creating {} failed: {e}", p.display())))?;
            Ok(p)
        };
        let run = mk(root.join("run"))?;
        let home = mk(root.join("home"))?;
        Ok(Self {
            socket_path: run.join("peekd.sock"),
            lock_path: run.join("peekd.lock"),
            support_dir: home.join("Library/Application Support/Peek"),
            caches_dir: home.join("Library/Caches/Peek"),
            applications_dir: home.join("Applications"),
            ui_executable: UiExecutableRule::PeekAppSuffix,
            launch_ui: false,
            commands: Arc::new(SystemCommands),
            telemetry_api: ApiUrl::production(),
            telemetry_enabled: false,
            timings: Timings::default(),
            bundle_id: BUNDLE_ID.to_owned(),
            own_build: own_build()?,
            honeycomb: HoneycombLocator {
                real_home: home.clone(),
                ..HoneycombLocator::default()
            },
            real_home: home,
            log_to_file: false,
            updates_enabled: false,
        })
    }

    /// `~/Applications/Peek.app`.
    #[must_use]
    pub fn app_path(&self) -> PathBuf {
        self.applications_dir.join("Peek.app")
    }
}

/// The directories an isolated run (decision 6) or the install hooks move.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IsolatedDirs {
    /// `~/Library/Application Support/Peek`, or its override.
    pub support: PathBuf,
    /// `~/Library/Caches/Peek`, or `PEEK_CACHES_DIR`.
    pub caches: PathBuf,
    /// `~/Applications`, or its override.
    pub applications: PathBuf,
    /// `PEEK_NO_SERVICES=1` or `PEEK_INSTALL_NO_LAUNCH=1`.
    pub isolated: bool,
}

impl IsolatedDirs {
    /// Resolves the overrides from `lookup` (the environment) exactly as the
    /// CLI does; empty values count as unset.
    pub fn resolve(real_home: &Path, lookup: impl Fn(&str) -> Option<OsString>) -> Self {
        let path = |k: &str| lookup(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        let flag = |k: &str| lookup(k).is_some_and(|v| v == "1");
        Self {
            support: path(INSTALL_HOOK_ENVS[0])
                .or_else(|| path(SUPPORT_DIR_ENV))
                .unwrap_or_else(|| real_home.join("Library/Application Support/Peek")),
            caches: path(CACHES_DIR_ENV).unwrap_or_else(|| real_home.join("Library/Caches/Peek")),
            applications: path(APPLICATIONS_DIR_ENV)
                .or_else(|| path(INSTALL_HOOK_ENVS[1]))
                .unwrap_or_else(|| real_home.join("Applications")),
            isolated: flag(NO_SERVICES_ENV) || flag(INSTALL_HOOK_ENVS[2]),
        }
    }
}

/// Whether `PEEK_NO_SERVICES=1` (an isolated run) or the install hook
/// `PEEK_INSTALL_NO_LAUNCH=1` (never `launchctl` or `open`) is set.
#[must_use]
pub fn no_services() -> bool {
    [NO_SERVICES_ENV, INSTALL_HOOK_ENVS[2]]
        .iter()
        .any(|k| std::env::var(k).is_ok_and(|v| v == "1"))
}

/// This build's `CFBundleVersion` (`major*1_000_000 + minor*1_000 + patch`),
/// equal to the app's `CURRENT_PROJECT_VERSION`.
///
/// # Errors
/// `internal_error` if the crate version is not `MAJOR.MINOR.PATCH`.
pub fn own_build() -> Result<u64> {
    AppOfferInfo::build_number(silicon_peek_client::VERSION).map_err(|e| {
        Error::new(
            ErrorCode::InternalError,
            format!(
                "this build's version cannot be a bundle version: {}",
                e.message()
            ),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isolated_dirs_follow_the_cli_and_the_install_hooks() {
        let home = Path::new("/Users/c");
        let resolve = |pairs: &[(&str, &str)]| {
            let map: std::collections::HashMap<String, OsString> = pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), OsString::from(v)))
                .collect();
            IsolatedDirs::resolve(home, move |k| map.get(k).cloned())
        };
        let default = resolve(&[]);
        assert_eq!(
            default.support,
            home.join("Library/Application Support/Peek")
        );
        assert_eq!(default.caches, home.join("Library/Caches/Peek"));
        assert_eq!(default.applications, home.join("Applications"));
        assert!(!default.isolated);
        let iso = resolve(&[
            ("PEEK_SUPPORT_DIR", "/r/support"),
            ("PEEK_CACHES_DIR", "/r/caches"),
            ("PEEK_NO_SERVICES", "1"),
        ]);
        assert_eq!(iso.support, Path::new("/r/support"));
        assert_eq!(iso.caches, Path::new("/r/caches"));
        assert!(iso.isolated);
        let hooks = resolve(&[
            ("PEEK_SUPPORT_DIR", "/r/support"),
            ("PEEK_INSTALL_SUPPORT_DIR", "/h/support"),
            ("PEEK_INSTALL_APPLICATIONS_DIR", "/h/Applications"),
            ("PEEK_INSTALL_NO_LAUNCH", "1"),
            ("PEEK_CACHES_DIR", ""),
        ]);
        assert_eq!(
            hooks.support,
            Path::new("/h/support"),
            "as the CLI resolves it"
        );
        assert_eq!(hooks.applications, Path::new("/h/Applications"));
        assert_eq!(
            hooks.caches,
            home.join("Library/Caches/Peek"),
            "empty is unset"
        );
        assert!(
            hooks.isolated,
            "PEEK_INSTALL_NO_LAUNCH=1 never launches anything"
        );
        assert!(
            !resolve(&[("PEEK_NO_SERVICES", "true")]).isolated,
            "only 1 counts"
        );
    }

    #[test]
    fn ui_rules() {
        let exact = UiExecutableRule::Exact(PathBuf::from("/Apps/Peek.app/Contents/MacOS/Peek"));
        assert!(exact.accepts(Path::new("/Apps/Peek.app/Contents/MacOS/Peek")));
        assert!(!exact.accepts(Path::new("/Other/Peek.app/Contents/MacOS/Peek")));
        let suffix = UiExecutableRule::PeekAppSuffix;
        assert!(suffix.accepts(Path::new("/x/y/Peek.app/Contents/MacOS/Peek")));
        assert!(!suffix.accepts(Path::new("/usr/bin/python3")));
        assert!(!suffix.accepts(Path::new("/x/NotPeek.app/Contents/MacOS/Peek")));
    }

    #[test]
    fn honeycomb_lookup_order() -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir()?;
        let registry_home = dir.path().join("silicon/.silicon/packages");
        let bin = registry_home.join(".honeycomb/dir/system/bin");
        std::fs::create_dir_all(&bin)?;
        let hc = bin.join("honeycomb");
        std::fs::write(&hc, b"#!/bin/sh\n")?;
        std::fs::set_permissions(&hc, std::fs::Permissions::from_mode(0o755))?;
        let loc = HoneycombLocator {
            env_override: None,
            path_var: Some(OsString::from("/nonexistent")),
            managed_prefix: Some(dir.path().join("prefix")),
            real_home: dir.path().join("home"),
        };
        assert_eq!(loc.resolve(&registry_home), Some(hc.clone()));
        assert_eq!(loc.resolve(dir.path()), None);
        let forced = HoneycombLocator {
            env_override: Some(PathBuf::from("/opt/hc")),
            ..loc
        };
        assert_eq!(forced.resolve(dir.path()), Some(PathBuf::from("/opt/hc")));
        Ok(())
    }

    #[test]
    fn rooted_layout() -> Result<()> {
        let dir = tempfile::tempdir().map_err(|e| Error::internal(e.to_string()))?;
        let c = DaemonConfig::rooted(dir.path())?;
        assert!(c.socket_path.starts_with(dir.path()));
        assert!(!c.launch_ui);
        assert_eq!(c.own_build, own_build()?);
        assert!(c.app_path().ends_with("Applications/Peek.app"));
        Ok(())
    }
}
