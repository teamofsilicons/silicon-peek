//! `peek doctor` (BLUEPRINT §7.6): every check, each with the exact fix.
//! A check that cannot run degrades to `warn` with the reason; the command
//! itself exits 0. The output holds no secrets, so `peek report
//! --attach-status` can include it.

use std::{path::PathBuf, process::Stdio, time::Duration};

use serde::Serialize;
use serde_json::{Value, json};
use silicon_peek_client::{
    http::Client,
    identity::ApiUrl,
    platform,
    runtime::{
        fs::verify_private_dir,
        session::{RELOGIN_HINT, SessionFile},
        sys,
    },
    timestamp::{Timestamp, unix_now},
};

use crate::{
    context::{Globals, Session},
    output::Out,
};

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Status {
    Ok,
    Warn,
    Fail,
}

#[derive(Clone, Debug, Serialize)]
struct Check {
    name: &'static str,
    status: Status,
    detail: String,
    fix: Option<String>,
}

fn check(
    name: &'static str,
    status: Status,
    detail: impl Into<String>,
    fix: Option<&str>,
) -> Check {
    Check {
        name,
        status,
        detail: detail.into(),
        fix: fix.map(str::to_owned),
    }
}

fn store_check() -> (Check, Option<PathBuf>) {
    match crate::context::store_dir() {
        Err(e) => (check("store", Status::Fail, e.message(), e.hint()), None),
        Ok(dir) if std::fs::symlink_metadata(&dir).is_err() => (
            check(
                "store",
                Status::Warn,
                format!("{} does not exist yet", dir.display()),
                Some("peek login '<SLT>' creates it"),
            ),
            None,
        ),
        Ok(dir) => match verify_private_dir(&dir) {
            Ok(()) => (
                check(
                    "store",
                    Status::Ok,
                    format!("{} is private to this user (0700)", dir.display()),
                    None,
                ),
                Some(dir),
            ),
            Err(e) => (
                check(
                    "store",
                    Status::Fail,
                    e.message(),
                    Some(&format!(
                        "chmod 700 '{}' && chmod 600 '{}'/*",
                        dir.display(),
                        dir.display()
                    )),
                ),
                None,
            ),
        },
    }
}

fn session_checks(session: &Session, file: &SessionFile) -> Vec<Check> {
    let mut checks = Vec::new();
    let now = unix_now();
    match file.slot(&session.slot_key) {
        None => {
            let logged_out = file.logged_out.is_some();
            checks.push(check(
                "session",
                Status::Warn,
                if logged_out {
                    format!("this home logged out of peek ({})", session.slot_key)
                } else {
                    format!("no peek session for {}", session.slot_key)
                },
                Some("iam silicon-login --app-id peek --grant-org <org> --approve-scopes; peek login '<SLT>'"),
            ));
        }
        Some(slot) => {
            if let Some(r) = &slot.rejected {
                checks.push(check(
                    "session",
                    Status::Fail,
                    format!(
                        "IAM rejected the session of {} ({}, at {})",
                        slot.actor.public_id,
                        r.code,
                        Timestamp::from_unix(r.at)
                    ),
                    Some(RELOGIN_HINT),
                ));
            } else if slot.reconsent_required {
                checks.push(check(
                    "session",
                    Status::Warn,
                    format!("{} lacks scopes peek needs (reconsent required)", slot.actor.public_id),
                    Some("log in again with --approve-scopes: iam silicon-login --app-id peek --grant-org <org> --approve-scopes; peek login '<SLT>'"),
                ));
            } else if slot.pending_refresh_key.is_some() {
                checks.push(check(
                    "session",
                    Status::Warn,
                    format!(
                        "a refresh of {}'s session is pending since {}",
                        slot.actor.public_id,
                        Timestamp::from_unix(slot.refresh_started_at.unwrap_or(now))
                    ),
                    Some("peek login status --json resumes it (within 10 minutes)"),
                ));
            } else {
                checks.push(check(
                    "session",
                    Status::Ok,
                    format!(
                        "{} in org {}; access token valid until {}",
                        slot.actor.public_id,
                        slot.org_id,
                        Timestamp::from_unix(slot.access_expires_at)
                    ),
                    None,
                ));
            }
            let subscribed = slot.ting.as_ref().is_some_and(|t| t.subscribed);
            checks.push(if subscribed {
                check(
                    "ting",
                    Status::Ok,
                    "enrolled as a Ting recipient for peek",
                    None,
                )
            } else {
                check(
                    "ting",
                    Status::Warn,
                    "not enrolled as a Ting recipient: answers cannot be delivered",
                    Some("peek ting enroll"),
                )
            });
        }
    }
    checks.extend(login_leftover_checks(file, now));
    checks
}

/// `pending_login` and `revocations`: always reported, `ok` when nothing is
/// pending.
fn login_leftover_checks(file: &SessionFile, now: i64) -> [Check; 2] {
    let pending_login = if file.pending_login_live(now) {
        check(
            "pending_login",
            Status::Warn,
            "an interrupted login can still be recovered",
            Some("peek login --recover"),
        )
    } else {
        check(
            "pending_login",
            Status::Ok,
            "no interrupted login to recover",
            None,
        )
    };
    let revocations = if file.pending_revocations.is_empty() {
        check(
            "revocations",
            Status::Ok,
            "no refresh-token revocation is waiting for the backend",
            None,
        )
    } else {
        check(
            "revocations",
            Status::Warn,
            format!(
                "{} refresh-token revocation(s) are queued for the backend",
                file.pending_revocations.len()
            ),
            Some("any peek login, logout or login status run retries them"),
        )
    };
    [pending_login, revocations]
}

async fn backend_check(client: &Client) -> Check {
    let api = client.api_url();
    match tokio::time::timeout(Duration::from_secs(6), client.readyz()).await {
        Err(_) => check(
            "backend",
            Status::Fail,
            format!("{api}/readyz did not answer within 6 s"),
            Some("check the network; retry later"),
        ),
        Ok(Err(e)) => check(
            "backend",
            Status::Fail,
            e.message(),
            Some(
                e.hint()
                    .unwrap_or("check the network and --api / PEEK_API_URL"),
            ),
        ),
        Ok(Ok(ready))
            if ready.status == "ready"
                && ready.checks.elevenlabs == "configured"
                && ready.checks.openai == "configured" =>
        {
            check("backend", Status::Ok, format!("{api} is ready"), None)
        }
        Ok(Ok(ready)) if ready.status == "ready" => check(
            "backend",
            Status::Warn,
            format!(
                "{} is ready; speech configuration: ElevenLabs TTS {}, OpenAI STT {}",
                api, ready.checks.elevenlabs, ready.checks.openai
            ),
            Some(
                "operators: configure PEEK_DEEPGRAM_API_KEY for speech and PEEK_OPENAI_API_KEY for transcription",
            ),
        ),
        Ok(Ok(ready)) => check(
            "backend",
            Status::Fail,
            format!(
                "{} is not ready (db {}, iam {}, ting {}, elevenlabs {}, openai {})",
                api,
                ready.checks.db,
                ready.checks.iam_config,
                ready.checks.ting_config,
                ready.checks.elevenlabs,
                ready.checks.openai
            ),
            Some("an operator issue; retry later, or report it with peek report"),
        ),
    }
}

fn honeycomb_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(p) = std::env::var_os("SILICON_HONEYCOMB").filter(|p| !p.is_empty()) {
        out.push(PathBuf::from(p));
        return out;
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let exe = dir.join(if cfg!(windows) {
                "honeycomb.exe"
            } else {
                "honeycomb"
            });
            if exe.is_file() {
                out.push(exe);
                break;
            }
        }
    }
    if let Some(home) = std::env::var_os("SILICON_HOME").filter(|p| !p.is_empty()) {
        out.push(PathBuf::from(home).join(".honeycomb/dir/system/bin/honeycomb"));
    }
    if let Ok(home) = sys::real_home() {
        out.push(home.join(".local/share/silicon/.honeycomb/dir/system/bin/honeycomb"));
        out.push(home.join(".honeycomb/dir/system/bin/honeycomb"));
    }
    out
}

fn parse_version(text: &str) -> Option<(u64, u64, u64, String)> {
    text.split(|c: char| {
        c.is_whitespace() || c == '"' || c == ',' || c == '{' || c == '}' || c == ':'
    })
    .map(|t| t.trim_start_matches('v'))
    .find_map(|t| {
        let mut parts = t.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch_raw = parts.next()?;
        let patch = patch_raw
            .split(|c: char| !c.is_ascii_digit())
            .next()?
            .parse()
            .ok()?;
        Some((major, minor, patch, t.to_owned()))
    })
}

async fn honeycomb_check() -> Check {
    let Some(exe) = honeycomb_candidates().into_iter().find(|p| p.is_file()) else {
        return check(
            "honeycomb",
            Status::Warn,
            "Honeycomb was not found (SILICON_HONEYCOMB, PATH, ~/.honeycomb/dir/system/bin)",
            Some("curl -fsSL https://peek.teamofsilicons.com/install.sh | sh"),
        );
    };
    let run = tokio::process::Command::new(&exe)
        .arg("--version")
        .env("HONEYCOMB_AUTO_UPDATE", "0")
        .env("HONEYCOMB_NO_SERVICE", "1")
        .env("HONEYCOMB_TELEMETRY", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output();
    let output = match tokio::time::timeout(Duration::from_secs(5), run).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            return check(
                "honeycomb",
                Status::Warn,
                format!("{} could not run: {e}", exe.display()),
                Some(
                    "reinstall Honeycomb: curl -fsSL https://peek.teamofsilicons.com/install.sh | sh",
                ),
            );
        }
        Err(_) => {
            return check(
                "honeycomb",
                Status::Warn,
                format!("{} --version did not finish within 5 s", exe.display()),
                None,
            );
        }
    };
    let text = String::from_utf8_lossy(&output.stdout);
    match parse_version(&text) {
        Some((major, minor, _, v)) if (major, minor) >= (0, 5) => check(
            "honeycomb",
            Status::Ok,
            format!("Honeycomb {v} at {}", exe.display()),
            None,
        ),
        Some((_, _, _, v)) => check(
            "honeycomb",
            Status::Warn,
            format!(
                "Honeycomb {v} at {} is older than 0.5.0 (no install scripts; peek installs Peek.app itself)",
                exe.display()
            ),
            Some("update Honeycomb (reinstall it with the peek installer, or update Stemcell)"),
        ),
        None => check(
            "honeycomb",
            Status::Warn,
            format!("could not read a version from {} --version", exe.display()),
            None,
        ),
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use std::time::Duration;

    use silicon_peek_client::{
        ErrorCode,
        ipc::{AuthBlock, cli::StatusOp},
        runtime::{auth_block, daemon::REQUEST_TIMEOUT},
    };

    use super::{Check, Status, check};
    use crate::{
        context::Session,
        service::{self, Paths, Service},
    };

    async fn app(paths: &Paths) -> Check {
        if std::fs::symlink_metadata(&paths.app).is_err() {
            return check(
                "app",
                Status::Warn,
                format!("Peek.app is not installed at {}", paths.app.display()),
                Some("peek app install"),
            );
        }
        let (build, short, id) = service::bundle_info(&paths.app).await;
        let id = id.unwrap_or_default();
        let team = if id == service::DEV_BUNDLE_ID {
            ""
        } else {
            service::TEAM_ID
        };
        match service::verify_signature(&paths.app, &id, team).await {
            Ok(true) => check(
                "app",
                Status::Ok,
                format!(
                    "Peek.app {} (build {}) at {}, signature valid",
                    short.unwrap_or_default(),
                    build.map_or_else(|| "?".to_owned(), |b| b.to_string()),
                    paths.app.display()
                ),
                None,
            ),
            Ok(false) => check(
                "app",
                Status::Fail,
                format!(
                    "{} fails codesign verification for {id}",
                    paths.app.display()
                ),
                Some("peek app uninstall; peek app install"),
            ),
            Err(e) => check("app", Status::Warn, e.message(), None),
        }
    }

    async fn agent() -> Check {
        match service::agent_loaded().await {
            Some(true) => check(
                "agent",
                Status::Ok,
                "the peekd launchd agent is loaded",
                None,
            ),
            Some(false) => check(
                "agent",
                Status::Warn,
                "the peekd launchd agent is not loaded (Peek.app never ran, or its background item awaits approval)",
                Some(
                    "open ~/Applications/Peek.app and allow Peek in System Settings → General → Login Items",
                ),
            ),
            None => check(
                "agent",
                Status::Warn,
                "not checked: talking to launchd is disabled (PEEK_INSTALL_NO_LAUNCH=1)",
                None,
            ),
        }
    }

    fn install_log(paths: &Paths) -> Check {
        let log = service::tail(&paths.install_status(), 20);
        match log.iter().rev().find(|l| l.starts_with("degraded")) {
            Some(line) => check(
                "install_log",
                Status::Warn,
                line.replace('\t', " "),
                Some("peek app install shows the full error"),
            ),
            None if log.is_empty() => check(
                "install_log",
                Status::Ok,
                "no install attempts logged",
                None,
            ),
            None => check(
                "install_log",
                Status::Ok,
                "the last install steps succeeded",
                None,
            ),
        }
    }

    /// Microphone and hotkeys, which only Peek.app knows (via peekd).
    async fn ui(svc: &mut Service, auth: Option<&AuthBlock>) -> Vec<Check> {
        /// Both UI checks as warnings with one explanation.
        fn unchecked(detail: &str, hint: Option<&str>) -> Vec<Check> {
            ["mic", "hotkeys"]
                .into_iter()
                .map(|n| check(n, Status::Warn, detail, hint))
                .collect()
        }
        let report = match svc
            .call(&service::DoctorOp {}, auth, Vec::new(), REQUEST_TIMEOUT)
            .await
        {
            Ok((report, _)) => report,
            Err(e) if *e.code() == ErrorCode::UnknownOp => {
                return unchecked(
                    "this peekd does not report it (older build)",
                    Some("peek app update"),
                );
            }
            Err(e) => return unchecked(&format!("not checked: {}", e.message()), e.hint()),
        };
        if report["mic"].is_null() && report["hotkeys"].is_null() {
            let why = report["ui_detail"]
                .as_str()
                .unwrap_or("Peek.app did not report it");
            return unchecked(
                &format!("not checked: {why}"),
                Some("open Peek (peek app install); run peek doctor again"),
            );
        }
        let mic = report["mic"].as_str().unwrap_or("unknown");
        let mic_check = match mic {
            "granted" | "authorized" => {
                check("mic", Status::Ok, "microphone access granted to Peek", None)
            }
            "denied" | "restricted" => check(
                "mic",
                Status::Fail,
                format!("microphone access is {mic}: voice answers cannot be recorded"),
                Some("System Settings → Privacy & Security → Microphone → allow Peek"),
            ),
            other => check(
                "mic",
                Status::Warn,
                format!(
                    "microphone access is {other} (Peek asks the first time the Carbon records)"
                ),
                None,
            ),
        };
        let failed: Vec<String> = report["hotkeys"]["failed"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|x| x.as_str().map_or_else(|| x.to_string(), str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let registered = report["hotkeys"]["registered"]
            .as_array()
            .map_or(0, Vec::len);
        let hotkeys = if failed.is_empty() {
            check(
                "hotkeys",
                Status::Ok,
                format!("{registered} hotkey(s) registered"),
                None,
            )
        } else {
            check(
                "hotkeys",
                Status::Warn,
                format!(
                    "hotkeys not registered: {} (another app holds them)",
                    failed.join(", ")
                ),
                Some(
                    "change the modifier in Peek → Settings, or free the shortcut in the other app",
                ),
            )
        };
        vec![mic_check, hotkeys]
    }

    async fn outbox(svc: &mut Service, auth: Option<&AuthBlock>) -> Check {
        let Some(auth) = auth else {
            return check(
                "outbox",
                Status::Warn,
                "not checked: this home is not logged in",
                Some("peek login '<SLT>'"),
            );
        };
        match svc
            .call(&StatusOp {}, Some(auth), Vec::new(), REQUEST_TIMEOUT)
            .await
        {
            Ok((st, _)) if st.deliveries.authority_required > 0 => check(
                "outbox",
                Status::Warn,
                format!(
                    "{} answer(s) wait for a valid session or Ting enrollment ({} pending{})",
                    st.deliveries.authority_required,
                    st.deliveries.pending,
                    st.deliveries
                        .last_error
                        .map(|e| format!(", last error {e}"))
                        .unwrap_or_default()
                ),
                Some("peek login status --json; peek ting enroll"),
            ),
            Ok((st, _)) if st.deliveries.pending > 0 => check(
                "outbox",
                Status::Warn,
                format!(
                    "{} delivery(ies) pending{}",
                    st.deliveries.pending,
                    st.deliveries
                        .last_error
                        .map(|e| format!(" (last error {e})"))
                        .unwrap_or_default()
                ),
                Some("they retry automatically; peek status shows progress"),
            ),
            Ok(_) => check("outbox", Status::Ok, "no deliveries waiting", None),
            Err(e) => check(
                "outbox",
                Status::Warn,
                format!("not checked: {}", e.message()),
                e.hint(),
            ),
        }
    }

    pub async fn checks(session: Option<&Session>) -> Vec<Check> {
        let Ok(paths) = service::paths() else {
            return vec![check(
                "app",
                Status::Fail,
                "cannot resolve this user's home directory",
                None,
            )];
        };
        let mut checks = vec![app(&paths).await, agent().await, install_log(&paths)];
        let auth = session.and_then(|s| auth_block(&s.store, &s.slot_key).ok());
        let mut svc = match service::connect_existing(Duration::from_secs(1)).await {
            Ok(Some(s)) => {
                checks.push(check(
                    "daemon",
                    Status::Ok,
                    format!(
                        "peekd {} answers on protocol {}",
                        s.hello.peekd_version, s.hello.protocol
                    ),
                    None,
                ));
                s
            }
            Ok(None) => {
                checks.push(check(
                    "daemon",
                    Status::Warn,
                    "peekd is not running",
                    Some("peek app install (or open ~/Applications/Peek.app)"),
                ));
                checks.extend(["mic", "hotkeys", "outbox"].into_iter().map(|n| {
                    check(
                        n,
                        Status::Warn,
                        "not checked: peekd is not running",
                        Some("peek app install"),
                    )
                }));
                return checks;
            }
            Err(e) => {
                checks.push(check("daemon", Status::Fail, e.message(), e.hint()));
                return checks;
            }
        };
        checks.extend(ui(&mut svc, auth.as_ref()).await);
        checks.push(outbox(&mut svc, auth.as_ref()).await);
        checks
    }
}

#[cfg(target_os = "macos")]
async fn mac_checks(session: Option<&Session>) -> Vec<Check> {
    mac::checks(session).await
}

#[cfg(not(target_os = "macos"))]
#[allow(clippy::unused_async)] // mirrors the macOS signature
async fn mac_checks(_session: Option<&Session>) -> Vec<Check> {
    vec![check(
        "app",
        Status::Ok,
        format!(
            "not needed: {} runs the IAM commands only; Peek.app is macOS-only",
            platform()
        ),
        None,
    )]
}

async fn collect(g: &Globals) -> Vec<Check> {
    let mut checks = Vec::new();
    let (store_check, store_dir) = store_check();
    checks.push(store_check);
    let mut session = None;
    if store_dir.is_some() {
        match crate::context::store() {
            Ok(store) => match g.session(store, false).await {
                Ok(s) => {
                    match s.store.read_session() {
                        Ok(file) => checks.extend(session_checks(&s, &file)),
                        Err(e) => {
                            checks.push(check("session", Status::Fail, e.message(), e.hint()));
                        }
                    }
                    session = Some(s);
                }
                Err(e) => checks.push(check("session", Status::Fail, e.message(), e.hint())),
            },
            Err(e) => checks.push(check("session", Status::Fail, e.message(), e.hint())),
        }
    }
    if !checks.iter().any(|c| c.name == "pending_login") {
        // No store or no readable session: nothing can be pending.
        checks.extend(login_leftover_checks(&SessionFile::default(), unix_now()));
    }
    if let Some(s) = &session {
        checks.push(backend_check(&s.client).await);
    } else {
        // No store yet: probe the backend without creating one.
        let api = g
            .explicit_api()
            .ok()
            .flatten()
            .unwrap_or_else(ApiUrl::production);
        match Client::builder(&api)
            .component(concat!("peek-cli/", env!("CARGO_PKG_VERSION")))
            .build()
        {
            Ok(client) => checks.push(backend_check(&client).await),
            Err(e) => checks.push(check("backend", Status::Fail, e.message(), e.hint())),
        }
    }
    checks.push(honeycomb_check().await);
    checks.extend(mac_checks(session.as_ref()).await);
    checks
}

/// The doctor result as JSON (also attached by `peek report --attach-status`).
pub async fn checks_value(g: &Globals) -> Value {
    let checks = collect(g).await;
    let count = |s: Status| checks.iter().filter(|c| c.status == s).count();
    json!({
        "checks": checks,
        "summary": {"ok": count(Status::Ok), "warn": count(Status::Warn), "fail": count(Status::Fail)},
        "version": silicon_peek_client::VERSION,
        "platform": platform(),
    })
}

pub async fn run(g: &Globals, out: Out) -> silicon_peek_client::Result<()> {
    let value = checks_value(g).await;
    out.value(&value, |v| {
        let mut lines = Vec::new();
        for c in v["checks"].as_array().into_iter().flatten() {
            let status = c["status"].as_str().unwrap_or_default();
            lines.push(format!(
                "[{status:<4}] {:<14} {}",
                c["name"].as_str().unwrap_or_default(),
                c["detail"].as_str().unwrap_or_default()
            ));
            if let Some(fix) = c["fix"].as_str() {
                lines.push(format!("{:21}fix: {fix}", ""));
            }
        }
        lines.push(format!(
            "{} ok, {} warn, {} fail",
            v["summary"]["ok"], v["summary"]["warn"], v["summary"]["fail"]
        ));
        lines.join("\n")
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn backend_checks_current_speech_providers() -> silicon_peek_client::Result<()> {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};
        let server = MockServer::start().await;
        let client = Client::builder(&ApiUrl::parse(&server.uri())?).build()?;
        for (openai, expected) in [("configured", Status::Ok), ("missing", Status::Warn)] {
            server.reset().await;
            Mock::given(path("/readyz"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "status": "ready", "checks": {
                        "db": "ok", "iam_config": "ok", "ting_config": "ok",
                        "elevenlabs": "configured", "openai": openai, "deepgram": "missing"
                    }
                })))
                .mount(&server)
                .await;
            let result = backend_check(&client).await;
            assert_eq!(result.status, expected);
            if expected == Status::Warn {
                assert!(
                    result
                        .fix
                        .as_deref()
                        .is_some_and(|s| s.contains("PEEK_OPENAI_API_KEY"))
                );
            }
        }
        Ok(())
    }

    #[test]
    fn versions_are_read_from_text_or_json() {
        assert_eq!(
            parse_version("honeycomb 0.5.0").map(|v| (v.0, v.1, v.2)),
            Some((0, 5, 0))
        );
        assert_eq!(
            parse_version(r#"{"version":"0.2.3"}"#).map(|v| (v.0, v.1, v.2)),
            Some((0, 2, 3))
        );
        assert_eq!(
            parse_version("v1.2.3-beta").map(|v| (v.0, v.1, v.2)),
            Some((1, 2, 3))
        );
        assert!(parse_version("no version").is_none());
    }
}
