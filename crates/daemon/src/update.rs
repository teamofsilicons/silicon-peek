//! Peek.app self-update and the stale-CLI watchdog (BLUEPRINT §4.6,
//! gap-honeycomb §4.2–§4.3).
//!
//! `update_once()` takes the newest offered build (offers written by the CLI,
//! `install-app.sh`, `app.offer`, or copied from a Honeycomb registry scan),
//! never downgrades, verifies the Developer ID requirement (a failing build
//! goes to `rejected.json`), waits until the UI is idle (an ask on screen is
//! never interrupted), quits the UI, swaps the bundle atomically with
//! `renamex_np(RENAME_SWAP)`, reopens it with `--after-update <old>` and
//! exits 0. Every external program goes through the injectable
//! [`CommandRunner`](crate::commands::CommandRunner).

use std::{
    collections::BTreeMap,
    fs::File,
    io::Read as _,
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering as AtomicOrdering},
    time::{Duration, Instant},
};

use rusqlite::{OptionalExtension as _, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use silicon_peek_client::{
    Error, ErrorCode, Result,
    ipc::{
        cli::{AppOffer, AppOfferInfo, AppOfferResult, BundledApp},
        ui::{AppQuit, AppUpdatePrepare, Restarting},
    },
    runtime::fs::{open_lock_file, read_private, write_atomic},
};

use crate::{
    bubbles::now_ms,
    commands::CommandSpec,
    config::{BUNDLE_ID, DEV_BUNDLE_ID, TEAM_ID},
    db::SqlResult as _,
    state::{Shared, SharedRef},
    sys,
    telemetry::Record,
};

/// Offers kept per channel (§4.3: the newest 3 builds).
pub const OFFERS_KEPT: usize = 3;

/// Reads a `<key>…</key><string>…</string>` value from an XML plist.
#[must_use]
pub fn plist_string(bytes: &[u8], key: &str) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    let needle = format!("<key>{key}</key>");
    let at = text.find(&needle)? + needle.len();
    let rest = text[at..].trim_start();
    let rest = rest.strip_prefix("<string>")?;
    let end = rest.find("</string>")?;
    Some(
        rest[..end]
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&amp;", "&"),
    )
}

/// The installed Peek.app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledApp {
    /// `CFBundleIdentifier`.
    pub bundle_id: String,
    /// `CFBundleVersion`.
    pub build: u64,
    /// `CFBundleShortVersionString`.
    pub short_version: String,
}

/// One stored offer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    /// Its sidecar.
    pub info: AppOfferInfo,
    /// `offers/<build>-<sha12>.app.zip`.
    pub zip: PathBuf,
}

/// What `update_once` did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateOutcome {
    /// `~/Applications/Peek.app` does not exist (the CLI installs the first
    /// copy; peekd only updates).
    NotInstalled,
    /// Nothing newer than the installed build.
    UpToDate {
        /// The installed build.
        installed: u64,
    },
    /// The best offer failed verification and was rejected.
    Rejected {
        /// Its build.
        build: u64,
        /// Why.
        reason: String,
    },
    /// The UI stayed busy; retried later.
    Deferred {
        /// The waiting build.
        build: u64,
    },
    /// Swapped; peekd exits.
    Applied {
        /// The old build.
        from: u64,
        /// The new build.
        to: u64,
    },
}

/// A Honeycomb context directory holding peek (for the registry scan and the
/// watchdog).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistryPeek {
    /// `<registry>/contexts/<fingerprint>`.
    pub context_dir: PathBuf,
    /// The home Honeycomb uses for this registry (`SILICON_HOME`).
    pub registry_home: PathBuf,
    /// The installed CLI version.
    pub version: String,
    /// The package directory.
    pub directory: PathBuf,
}

fn file_sha256(path: &Path) -> std::io::Result<String> {
    let mut f = File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

fn sidecar(info: &AppOfferInfo) -> String {
    format!(
        "bundle_id={}\nbundle_version={}\nshort_version={}\nteam_id={}\nzip_sha256={}\nminimum_system_version={}\n",
        info.bundle_id,
        info.bundle_version,
        info.short_version,
        info.team_id,
        info.zip_sha256,
        info.minimum_system_version
    )
}

fn version_build(v: &str) -> Option<u64> {
    AppOfferInfo::build_number(v.trim_start_matches('v')).ok()
}

impl Shared {
    fn signing_requirement() -> String {
        format!(
            "anchor apple generic and identifier \"{BUNDLE_ID}\" and certificate 1[field.1.2.840.113635.100.6.2.6] and certificate leaf[field.1.2.840.113635.100.6.1.13] and certificate leaf[subject.OU] = \"{TEAM_ID}\""
        )
    }

    /// Reads the installed app's Info.plist (XML directly; binary through
    /// `plutil`).
    pub async fn installed_app(&self) -> Option<InstalledApp> {
        let plist = self.cfg.app_path().join("Contents/Info.plist");
        let raw = std::fs::read(&plist).ok()?;
        let xml = if raw.starts_with(b"bplist") {
            let spec = CommandSpec::new("/usr/bin/plutil")
                .arg("-convert")
                .arg("xml1")
                .arg("-o")
                .arg("-")
                .arg(plist.as_os_str())
                .timeout(Duration::from_secs(10));
            let out = self.cfg.commands.run(spec).await.ok()?;
            if !out.success() {
                return None;
            }
            out.stdout
        } else {
            raw
        };
        Some(InstalledApp {
            bundle_id: plist_string(&xml, "CFBundleIdentifier")?,
            build: plist_string(&xml, "CFBundleVersion")?.trim().parse().ok()?,
            short_version: plist_string(&xml, "CFBundleShortVersionString").unwrap_or_default(),
        })
    }

    /// The installed build, or this peekd's own build when the app is not in
    /// `~/Applications` (development runs).
    pub async fn installed_build(&self) -> u64 {
        self.installed_app()
            .await
            .map_or(self.cfg.own_build, |a| a.build)
    }

    fn rejected(&self) -> BTreeMap<String, String> {
        read_private(&self.paths.rejected())
            .ok()
            .flatten()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn reject(&self, sha: &str, reason: &str) {
        let mut map = self.rejected();
        map.insert(sha.to_owned(), reason.to_owned());
        let mut bytes = serde_json::to_vec_pretty(&map).unwrap_or_default();
        bytes.push(b'\n');
        if let Err(e) = write_atomic(&self.paths.support, "rejected.json", &bytes) {
            tracing::warn!(error = %e, "writing rejected.json failed");
        }
    }

    /// Every stored offer for this channel.
    #[must_use]
    pub fn offers(&self) -> Vec<Offer> {
        let Ok(rd) = std::fs::read_dir(self.paths.offers_dir()) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(stem) = name.strip_suffix(".app.info") else {
                continue;
            };
            let Ok(text) = std::fs::read_to_string(&p) else {
                continue;
            };
            let Ok(info) = AppOfferInfo::parse_sidecar(&text) else {
                continue;
            };
            let zip = self.paths.offers_dir().join(format!("{stem}.app.zip"));
            if info.bundle_id == self.cfg.bundle_id && zip.is_file() {
                out.push(Offer { info, zip });
            }
        }
        out.sort_by(|a, b| {
            (a.info.bundle_version, &a.info.zip_sha256)
                .cmp(&(b.info.bundle_version, &b.info.zip_sha256))
        });
        out
    }

    /// Copies an offered zip (checking its SHA-256) into `offers/`. Returns
    /// whether a new offer was stored.
    ///
    /// # Errors
    /// `invalid_input` for a missing zip, a wrong channel or a hash mismatch.
    pub fn store_offer(&self, zip_path: &Path, info: &AppOfferInfo) -> Result<bool> {
        // `app.offer` arrives as plain serde: nothing about its fields is
        // trusted before this (a short hash used to panic the slice below,
        // and panics abort peekd).
        info.validate()?;
        let sha256 = info.zip_sha256.to_ascii_lowercase();
        if info.bundle_id != self.cfg.bundle_id {
            return Err(Error::invalid_input(format!(
                "the offered Peek.app is `{}`; this peekd updates `{}` only",
                info.bundle_id, self.cfg.bundle_id
            )));
        }
        if !zip_path.is_absolute() || !zip_path.is_file() {
            return Err(Error::invalid_input(format!(
                "the offered zip {} is not an existing absolute path",
                zip_path.display()
            )));
        }
        let stem = format!(
            "{}-{}",
            info.bundle_version,
            sha256.get(..12).unwrap_or(&sha256)
        );
        let dir = self.paths.offers_dir();
        let dest = dir.join(format!("{stem}.app.zip"));
        if dest.is_file() && dir.join(format!("{stem}.app.info")).is_file() {
            return Ok(false);
        }
        let actual = file_sha256(zip_path).map_err(|e| {
            Error::invalid_input(format!("reading {} failed: {e}", zip_path.display()))
        })?;
        if actual != sha256 {
            return Err(Error::invalid_input(format!(
                "the offered zip {} has SHA-256 {actual}, but its Peek.app.info says {}",
                zip_path.display(),
                info.zip_sha256
            )));
        }
        let tmp = dir.join(format!(".{stem}.{}.tmp", uuid::Uuid::now_v7().simple()));
        std::fs::copy(zip_path, &tmp)
            .and_then(|_| std::fs::rename(&tmp, &dest))
            .map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                Error::internal(format!(
                    "copying the offer into {} failed: {e}",
                    dest.display()
                ))
            })?;
        write_atomic(&dir, &format!("{stem}.app.info"), sidecar(info).as_bytes())?;
        self.prune_offers();
        Ok(true)
    }

    fn prune_offers(&self) {
        let offers = self.offers();
        let mut builds: Vec<u64> = offers.iter().map(|o| o.info.bundle_version).collect();
        builds.dedup();
        if builds.len() <= OFFERS_KEPT {
            return;
        }
        let cutoff = builds[builds.len() - OFFERS_KEPT];
        for o in offers.iter().filter(|o| o.info.bundle_version < cutoff) {
            let _ = std::fs::remove_file(&o.zip);
            let info = o.zip.with_file_name(
                o.zip
                    .file_name()
                    .map(|n| n.to_string_lossy().replace(".app.zip", ".app.info"))
                    .unwrap_or_default(),
            );
            let _ = std::fs::remove_file(info);
        }
    }

    /// Honeycomb registries to scan: the Carbon's own and every known
    /// Silicon home's package registry.
    pub async fn registries(&self) -> Vec<(PathBuf, PathBuf)> {
        let mut out = vec![(
            self.cfg.real_home.join(".honeycomb/dir"),
            self.cfg.real_home.clone(),
        )];
        for h in self.known_homes().await.unwrap_or_default() {
            let packages = Path::new(&h.silicon_home).join(".silicon/packages");
            let reg = packages.join(".honeycomb/dir");
            if !out.iter().any(|(r, _)| *r == reg) {
                out.push((reg, packages));
            }
        }
        out
    }

    /// Every Honeycomb context holding peek.
    pub async fn registry_scan(&self) -> Vec<RegistryPeek> {
        let mut found = Vec::new();
        for (registry, home) in self.registries().await {
            let Ok(rd) = std::fs::read_dir(registry.join("contexts")) else {
                continue;
            };
            for e in rd.flatten() {
                let dir = e.path();
                let Ok(bytes) = std::fs::read(dir.join("installed.json")) else {
                    continue;
                };
                let Ok(v) = serde_json::from_slice::<Value>(&bytes) else {
                    continue;
                };
                let Some(rec) = v.get("peek") else { continue };
                let (Some(version), Some(directory)) = (
                    rec.get("version").and_then(Value::as_str),
                    rec.get("directory").and_then(Value::as_str),
                ) else {
                    continue;
                };
                found.push(RegistryPeek {
                    context_dir: dir.clone(),
                    registry_home: home.clone(),
                    version: version.to_owned(),
                    directory: PathBuf::from(directory),
                });
            }
        }
        found
    }

    /// Copies newer builds found in registries into `offers/` (Honeycomb
    /// deletes package directories on the next update).
    pub async fn scan_registries_for_offers(&self) {
        let installed = self.installed_build().await;
        for r in self.registry_scan().await {
            let info_path = r.directory.join("Peek.app.info");
            let zip = r.directory.join("Peek.app.zip");
            let Ok(text) = std::fs::read_to_string(&info_path) else {
                continue;
            };
            let Ok(info) = AppOfferInfo::parse_sidecar(&text) else {
                continue;
            };
            if info.bundle_version > installed
                && info.bundle_id == self.cfg.bundle_id
                && let Err(e) = self.store_offer(&zip, &info)
            {
                tracing::info!(error = %e, zip = %zip.display(), "a registry offer could not be stored");
            }
        }
    }

    async fn lock_install(&self) -> Result<File> {
        let file = open_lock_file(&self.paths.install_lock())?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(file),
                Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(std::fs::TryLockError::WouldBlock) => {
                    return Err(Error::new(
                        ErrorCode::AppUpdatePending,
                        "another installer held install.lock for 30 s",
                    ));
                }
                Err(std::fs::TryLockError::Error(e)) => {
                    return Err(Error::internal(format!("locking install.lock failed: {e}")));
                }
            }
        }
    }

    async fn run_checked(&self, spec: CommandSpec) -> std::result::Result<(), String> {
        let name = spec.name();
        match self.cfg.commands.run(spec).await {
            Ok(out) if out.success() => Ok(()),
            Ok(out) => Err(format!("{name}: {}", out.last_line())),
            Err(e) => Err(format!("{name}: {e}")),
        }
    }

    /// Extracts and verifies an offer into a stage directory next to the app.
    async fn stage(&self, offer: &Offer) -> std::result::Result<PathBuf, String> {
        let actual =
            file_sha256(&offer.zip).map_err(|e| format!("reading the offer failed: {e}"))?;
        if actual != offer.info.zip_sha256 {
            return Err(format!(
                "the stored zip's SHA-256 is {actual}, not {}",
                offer.info.zip_sha256
            ));
        }
        let stage = self
            .cfg
            .applications_dir
            .join(format!(".Peek.app.update.{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&stage);
        std::fs::create_dir_all(&stage)
            .map_err(|e| format!("creating {} failed: {e}", stage.display()))?;
        self.run_checked(
            CommandSpec::new("/usr/bin/ditto")
                .arg("-x")
                .arg("-k")
                .arg(offer.zip.as_os_str())
                .arg(stage.as_os_str())
                .timeout(Duration::from_secs(300)),
        )
        .await?;
        let app = stage.join("Peek.app");
        if !app.is_dir() {
            return Err("the zip does not contain Peek.app at its top level".to_owned());
        }
        let dev = offer.info.team_id.is_empty() && offer.info.bundle_id == DEV_BUNDLE_ID;
        let mut verify = CommandSpec::new("/usr/bin/codesign")
            .arg("--verify")
            .arg("--deep")
            .arg("--strict");
        if !dev {
            verify = verify.arg(format!("-R={}", Self::signing_requirement()));
        }
        self.run_checked(
            verify
                .arg(app.as_os_str())
                .timeout(Duration::from_secs(300)),
        )
        .await
        .map_err(|e| format!("the code signature does not verify: {e}"))?;
        let _ = self
            .run_checked(
                CommandSpec::new("/usr/bin/xattr")
                    .arg("-dr")
                    .arg("com.apple.quarantine")
                    .arg(app.as_os_str()),
            )
            .await;
        Ok(stage)
    }

    /// Waits until Peek.app is idle (no bubble, recording or ask on screen),
    /// polling for up to the deferral cap.
    async fn wait_idle(&self, build: u64) -> bool {
        let deadline = Instant::now() + self.cfg.timings.update_defer_max;
        loop {
            if !self.any_ask_on_screen().await {
                if !self.ui.is_connected() {
                    return true;
                }
                match self
                    .ui
                    .request(
                        &AppUpdatePrepare { build },
                        Vec::new(),
                        self.cfg.timings.ui_request_timeout,
                    )
                    .await
                {
                    Ok((r, _)) if r.ready => return true,
                    Ok(_) => {}
                    Err(e)
                        if *e.code() == ErrorCode::PeekServiceUnavailable
                            && !self.ui.is_connected() =>
                    {
                        return true;
                    }
                    Err(e) => tracing::info!(error = %e, "app.update.prepare failed; deferring"),
                }
            }
            if Instant::now() + self.cfg.timings.update_defer_poll > deadline
                || self.shutting_down()
            {
                return false;
            }
            let mut shutdown = self.shutdown.subscribe();
            tokio::select! {
                () = tokio::time::sleep(self.cfg.timings.update_defer_poll) => {}
                _ = shutdown.changed() => return false,
            }
        }
    }

    /// The last check before quitting Peek.app, with new pushes already held
    /// and the install lock taken: still no ask on screen, and the UI still
    /// says it is idle.
    async fn still_idle(&self, build: u64) -> bool {
        if self.any_ask_on_screen().await {
            return false;
        }
        if !self.ui.is_connected() {
            return true;
        }
        match self
            .ui
            .request(
                &AppUpdatePrepare { build },
                Vec::new(),
                self.cfg.timings.ui_request_timeout,
            )
            .await
        {
            Ok((r, _)) => r.ready,
            Err(e) => *e.code() == ErrorCode::PeekServiceUnavailable && !self.ui.is_connected(),
        }
    }

    /// Asks the UI to quit and waits for it to disconnect (SIGTERM after the
    /// grace period). Returns false, without quitting anything, when the UI
    /// answers `app.quit` with `ready:false` (it is busy after all): an ask
    /// on screen is never interrupted. SIGTERM is only for a UI that agreed
    /// but did not exit, or that does not answer at all.
    async fn quit_ui(&self, build: u64) -> bool {
        let Some(link) = self.ui.current() else {
            return true;
        };
        self.ui.event(&Restarting { to_build: build }, Vec::new());
        if let Ok((r, _)) = self
            .ui
            .request(
                &AppQuit { build },
                Vec::new(),
                self.cfg.timings.ui_request_timeout,
            )
            .await
            && !r.ready
        {
            tracing::info!(
                build,
                "Peek.app is busy and declined to quit; the update waits"
            );
            return false;
        }
        let deadline = Instant::now() + self.cfg.timings.ui_quit_wait;
        while Instant::now() < deadline {
            if self.ui.current().is_none_or(|l| l.conn_id != link.conn_id) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if let Some(pid) = link
            .pid
            .filter(|p| u32::try_from(*p).ok() != Some(std::process::id()))
        {
            tracing::warn!(pid, "Peek.app did not quit for the update; sending SIGTERM");
            let _ = sys::terminate(pid);
            let end = Instant::now() + Duration::from_secs(3);
            while sys::process_alive(pid) && Instant::now() < end {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        true
    }

    /// One update attempt (§4.6).
    ///
    /// # Errors
    /// `app_update_pending` when the install lock is busy; I/O failures.
    #[allow(clippy::too_many_lines)] // one linear procedure: offer → stage → idle → swap → reopen
    pub async fn update_once(self: &SharedRef) -> Result<UpdateOutcome> {
        let Some(installed) = self.installed_app().await else {
            return Ok(UpdateOutcome::NotInstalled);
        };
        let lock = self.lock_install().await?;
        self.scan_registries_for_offers().await;
        let rejected = self.rejected();
        let best = self
            .offers()
            .into_iter()
            .filter(|o| !rejected.contains_key(&o.info.zip_sha256))
            .max_by(|a, b| {
                (a.info.bundle_version, &a.info.zip_sha256)
                    .cmp(&(b.info.bundle_version, &b.info.zip_sha256))
            });
        let Some(best) = best.filter(|o| o.info.bundle_version > installed.build) else {
            return Ok(UpdateOutcome::UpToDate {
                installed: installed.build,
            });
        };
        let to = best.info.bundle_version;
        let stage = match self.stage(&best).await {
            Ok(s) => s,
            Err(reason) => {
                tracing::warn!(build = to, %reason, "rejecting a Peek.app offer");
                self.reject(&best.info.zip_sha256, &reason);
                let _ = std::fs::remove_dir_all(
                    self.cfg
                        .applications_dir
                        .join(format!(".Peek.app.update.{}", std::process::id())),
                );
                self.record(
                    Record::new("app.update", "error")
                        .with("update_from", installed.build)
                        .with("update_to", to),
                );
                return Ok(UpdateOutcome::Rejected { build: to, reason });
            }
        };
        // Never hold the lock for minutes while waiting for the UI.
        drop(lock);
        if !self.wait_idle(to).await {
            let _ = std::fs::remove_dir_all(&stage);
            return Ok(UpdateOutcome::Deferred { build: to });
        }
        // From here until the swap, new bubbles are held back (the guard
        // releases them if the update backs off or fails).
        let held = HeldPushes::hold(self);
        let lock = match self.lock_install().await {
            Ok(l) => l,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&stage);
                return Err(e);
            }
        };
        if self.installed_app().await.map(|a| a.build) != Some(installed.build) {
            drop(lock);
            drop(held);
            let _ = std::fs::remove_dir_all(&stage);
            return Box::pin(self.update_once()).await;
        }
        // Waiting for the lock and reading Info.plist took time: an ask may
        // have reached the screen meanwhile. Check again, then quit only a
        // UI that agrees.
        if !self.still_idle(to).await || !self.quit_ui(to).await {
            drop(lock);
            drop(held);
            let _ = std::fs::remove_dir_all(&stage);
            return Ok(UpdateOutcome::Deferred { build: to });
        }
        let app = self.cfg.app_path();
        sys::rename_swap(&stage.join("Peek.app"), &app).map_err(|e| {
            let _ = std::fs::remove_dir_all(&stage);
            Error::internal(format!("swapping {} into place failed: {e}", app.display()))
        })?;

        let _ = std::fs::remove_dir_all(&stage);
        let applied = json!({"from": installed.build, "to": to, "at": silicon_peek_client::timestamp::Timestamp::now()});
        let mut bytes = serde_json::to_vec_pretty(&applied).unwrap_or_default();
        bytes.push(b'\n');
        write_atomic(&self.paths.support, "update-applied.json", &bytes)?;
        drop(lock);
        self.record(
            Record::new("app.update", "ok")
                .with("update_from", installed.build)
                .with("update_to", to),
        );
        let reopen = CommandSpec::new("/usr/bin/open")
            .arg("-g")
            .arg("-j")
            .arg(app.as_os_str())
            .arg("--args")
            .arg("--after-update")
            .arg(installed.build.to_string())
            .timeout(Duration::from_secs(15));
        if let Err(e) = self.run_checked(reopen).await {
            tracing::warn!(error = %e, "reopening Peek.app after the update failed");
        }
        tracing::info!(
            from = installed.build,
            to,
            "Peek.app updated; peekd exits so the new one starts"
        );
        // peekd exits: the queue is persisted and the next peekd shows it on
        // the new UI, so nothing is released here.
        held.keep();
        self.request_exit(0);
        Ok(UpdateOutcome::Applied {
            from: installed.build,
            to,
        })
    }

    /// `app.offer`.
    ///
    /// # Errors
    /// As [`Shared::store_offer`].
    pub async fn handle_app_offer(self: &SharedRef, op: &AppOffer) -> Result<AppOfferResult> {
        let this = Arc::clone(self);
        let zip = PathBuf::from(&op.zip_path);
        let info = op.info.clone();
        tokio::task::spawn_blocking(move || this.store_offer(&zip, &info))
            .await
            .map_err(|e| Error::internal(format!("storing an offer failed: {e}")))??;
        let installed = self.installed_build().await;
        let scheduled = op.info.bundle_version > installed;
        if scheduled {
            self.update_wake.notify_one();
        }
        Ok(AppOfferResult {
            scheduled,
            installed_build: installed,
        })
    }

    /// A CLI's `hello.bundled_app` newer than the installed build is treated
    /// as an offer (its `Peek.app.info` sits next to the zip).
    pub fn offer_from_hello(self: &SharedRef, bundled: &BundledApp) {
        let this = Arc::clone(self);
        let bundled = bundled.clone();
        tokio::spawn(async move {
            if bundled.build <= this.installed_build().await {
                return;
            }
            let zip = PathBuf::from(&bundled.zip_path);
            let Ok(text) = std::fs::read_to_string(zip.with_extension("info")) else {
                return;
            };
            let Ok(info) = AppOfferInfo::parse_sidecar(&text) else {
                return;
            };
            if info.bundle_version != bundled.build || info.zip_sha256 != bundled.zip_sha256 {
                return;
            }
            let t2 = Arc::clone(&this);
            let stored = tokio::task::spawn_blocking(move || t2.store_offer(&zip, &info)).await;
            if matches!(stored, Ok(Ok(true))) {
                this.update_wake.notify_one();
            }
        });
    }

    /// The hourly stale-CLI watchdog (§4.6): a registry whose peek has lagged
    /// the newest release for more than 2 h gets `honeycomb update peek`.
    ///
    /// # Errors
    /// Database failures.
    pub async fn cli_watchdog(self: &SharedRef) -> Result<usize> {
        if !self.settings.get().updates.cli_watchdog {
            return Ok(0);
        }
        let scan = self.registry_scan().await;
        let newest = scan
            .iter()
            .filter_map(|r| version_build(&r.version).map(|b| (b, r.version.clone())))
            .chain(
                self.offers()
                    .iter()
                    .map(|o| (o.info.bundle_version, o.info.short_version.clone())),
            )
            .chain(std::iter::once((
                self.cfg.own_build,
                silicon_peek_client::VERSION.to_owned(),
            )))
            .max_by_key(|(b, _)| *b);
        let Some((newest_build, newest_version)) = newest else {
            return Ok(0);
        };
        let now = now_ms();
        let behind_ms =
            i64::try_from(self.cfg.timings.watchdog_behind.as_millis()).unwrap_or(i64::MAX);
        let mut ran = 0;
        for r in scan {
            let key = r.context_dir.to_string_lossy().into_owned();
            let behind = version_build(&r.version).is_some_and(|b| b < newest_build);
            if !behind {
                let k = key.clone();
                self.db
                    .call(move |c| {
                        c.execute("DELETE FROM cli_watchdog WHERE context_dir = ?1", [k])
                            .sql()
                    })
                    .await?;
                continue;
            }
            let k = key.clone();
            let (since, last_run): (i64, Option<i64>) = self
                .db
                .call(move |c| {
                    c.execute(
                        "INSERT OR IGNORE INTO cli_watchdog (context_dir, behind_since) VALUES (?1, ?2)",
                        params![k, now],
                    )
                    .sql()?;
                    c.query_row(
                        "SELECT behind_since, last_run_at FROM cli_watchdog WHERE context_dir = ?1",
                        [k],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()
                    .sql()
                    .map(|o| o.unwrap_or((now, None)))
                })
                .await?;
            let hour = 3_600_000;
            if now.saturating_sub(since) < behind_ms
                || last_run.is_some_and(|l| now.saturating_sub(l) < hour)
            {
                continue;
            }
            let Some(honeycomb) = self.cfg.honeycomb.resolve(&r.registry_home) else {
                tracing::warn!(registry = %r.registry_home.display(), "the CLI watchdog found no Honeycomb binary");
                continue;
            };
            let spec = CommandSpec::new(honeycomb)
                .arg("update")
                .arg("peek")
                .arg("--json")
                .env("HONEYCOMB_NO_SERVICE", "1")
                .env("HONEYCOMB_NO_MODIFY_PATH", "1")
                .env("SILICON_HOME", r.registry_home.as_os_str())
                .timeout(Duration::from_secs(300));
            let outcome = self.run_checked(spec).await;
            ran += 1;
            let ok = outcome.is_ok();
            let detail = outcome.err().unwrap_or_else(|| "ok".to_owned());
            tracing::info!(registry = %r.registry_home.display(), from = %r.version, to = %newest_version, %detail, "CLI watchdog ran honeycomb update");
            let k = key.clone();
            self.db
                .call(move |c| {
                    c.execute(
                        "UPDATE cli_watchdog SET last_run_at = ?2, last_outcome = ?3 WHERE context_dir = ?1",
                        params![k, now, detail],
                    )
                    .sql()
                })
                .await?;
            self.record(
                Record::new("update.cli_watchdog", if ok { "ok" } else { "error" })
                    .with("update_from", r.version.clone())
                    .with("update_to", newest_version.clone()),
            );
        }
        Ok(ran)
    }

    /// At start (after a delay), hourly, and on a new offer: `update_once`.
    /// It may wait hours for a busy UI, so the CLI watchdog and revocation
    /// sweeps run on their own schedule ([`Shared::run_maintenance`]).
    pub async fn run_updates(self: SharedRef) {
        let mut shutdown = self.shutdown.subscribe();
        let mut wait = self.cfg.timings.update_initial_delay;
        loop {
            if self.shutting_down() {
                return;
            }
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                () = self.update_wake.notified() => {}
                _ = shutdown.changed() => return,
            }
            if self.shutting_down() {
                return;
            }
            match self.update_once().await {
                Ok(UpdateOutcome::Applied { .. }) => return,
                Ok(o) => tracing::debug!(outcome = ?o, "update check"),
                Err(e) => tracing::warn!(error = %e, "the update check failed"),
            }
            wait = self.cfg.timings.update_interval;
        }
    }

    /// Hourly (after the same start delay): the CLI watchdog (§4.6) and the
    /// retry of pending refresh-token revocations. Independent of
    /// `update_once`, which can defer for up to 6 h behind a busy UI.
    pub async fn run_maintenance(self: SharedRef) {
        let mut shutdown = self.shutdown.subscribe();
        let mut wait = self.cfg.timings.update_initial_delay;
        loop {
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                _ = shutdown.changed() => return,
            }
            if self.shutting_down() {
                return;
            }
            if let Err(e) = self.cli_watchdog().await {
                tracing::warn!(error = %e, "the CLI watchdog failed");
            }
            for h in self.known_homes().await.unwrap_or_default() {
                self.sweep_revocations(&h.home_path).await;
            }
            wait = self.cfg.timings.update_interval;
        }
    }
}

/// Holds new bubble pushes while an update is about to quit Peek.app; on
/// drop (the update backed off or failed) releases them and shows whatever
/// arrived meanwhile.
struct HeldPushes {
    shared: SharedRef,
    release: bool,
}

impl HeldPushes {
    fn hold(shared: &SharedRef) -> Self {
        shared.update_swapping.store(true, AtomicOrdering::SeqCst);
        Self {
            shared: Arc::clone(shared),
            release: true,
        }
    }

    /// The swap happened: peekd exits, nothing is released.
    fn keep(mut self) {
        self.release = false;
    }
}

impl Drop for HeldPushes {
    fn drop(&mut self) {
        if !self.release {
            return;
        }
        self.shared
            .update_swapping
            .store(false, AtomicOrdering::SeqCst);
        let shared = Arc::clone(&self.shared);
        tokio::spawn(async move { shared.push_all().await });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_values() {
        let xml = br#"<?xml version="1.0"?><plist><dict>
            <key>CFBundleIdentifier</key>
            <string>ai.tos.peek</string>
            <key>CFBundleVersion</key><string>1002</string>
            <key>Odd</key><string>a &amp; b</string></dict></plist>"#;
        assert_eq!(
            plist_string(xml, "CFBundleIdentifier").as_deref(),
            Some("ai.tos.peek")
        );
        assert_eq!(
            plist_string(xml, "CFBundleVersion").as_deref(),
            Some("1002")
        );
        assert_eq!(plist_string(xml, "Odd").as_deref(), Some("a & b"));
        assert_eq!(plist_string(xml, "Missing"), None);
    }

    #[test]
    fn versions() {
        assert_eq!(version_build("0.1.0"), Some(1000));
        assert_eq!(version_build("v1.2.3"), Some(1_002_003));
        assert_eq!(version_build("dev"), None);
    }
}
