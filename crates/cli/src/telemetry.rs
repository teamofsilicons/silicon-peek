//! CLI telemetry: one `command.finished` event per invocation (BLUEPRINT
//! §6.3–§6.6).
//!
//! The event is handed to peekd over IPC (`telemetry` op) when peekd is
//! already running and this home is logged in; otherwise it is posted to the
//! backend gateway `POST /api/web/telemetry` with a 300 ms timeout and dropped
//! on failure. It never blocks a command for longer than that, never starts
//! peekd, and is skipped entirely for `peek iam` (which must have no side
//! effects). Any opt-out wins: `--no-telemetry`, `PEEK_TELEMETRY`,
//! `SPACE_STATION_TELEMETRY`, `SILICON_TELEMETRY` (0/false/off/no) or the
//! home's config `telemetry:false`. No text, path, token or raw actor id is
//! ever recorded; actors are hashed.
//!
//! peekd works for the home long after a command returns (deliveries,
//! speech, drawing syncs), so an environment opt-out is forwarded: a run
//! that talked to peekd for its home and whose environment opts out ends
//! with a `config.sync` carrying `telemetry:false`. peekd keeps honouring it
//! until a run of that home hands it telemetry again (or `peek config …`
//! syncs without the opt-out). `--no-telemetry` stays with its process.

use std::{
    sync::{Mutex, OnceLock},
    time::Duration,
};

use serde_json::{Value, json};
use silicon_peek_client::{
    Error,
    api::{TelemetryBatch, TelemetryEvent, TelemetryTable},
    http::Client,
    identity::{ActorId, ApiUrl, Context, OrgId},
    ipc::AuthBlock,
    telemetry::actor_hash,
    timestamp::Timestamp,
};
use uuid::Uuid;

use crate::context::Globals;

/// Budget for the direct gateway post.
const POST_TIMEOUT: Duration = Duration::from_millis(300);
/// Budget for the IPC hand-off to a running peekd.
#[cfg(target_os = "macos")]
const IPC_TIMEOUT: Duration = Duration::from_millis(200);

static TRACE_ID: OnceLock<String> = OnceLock::new();
static INSTANCE_ID: OnceLock<String> = OnceLock::new();

/// The per-invocation trace id (a `UUIDv7`), also sent as `X-Peek-Trace-Id`.
pub fn trace_id() -> String {
    TRACE_ID
        .get_or_init(|| Uuid::now_v7().hyphenated().to_string())
        .clone()
}

fn instance_id() -> String {
    INSTANCE_ID
        .get_or_init(|| Uuid::now_v7().hyphenated().to_string())
        .clone()
}

#[derive(Default)]
struct Info {
    enabled: Option<bool>,
    client: Option<Client>,
    context: Option<Context>,
    actor: Option<(&'static str, String)>,
    auth: Option<AuthBlock>,
}

static INFO: OnceLock<Mutex<Info>> = OnceLock::new();

fn with_info(f: impl FnOnce(&mut Info)) {
    if let Ok(mut info) = INFO.get_or_init(|| Mutex::new(Info::default())).lock() {
        f(&mut info);
    }
}

/// Records the session's telemetry decision and client.
pub fn note_session(enabled: bool, client: &Client, context: Context) {
    with_info(|i| {
        i.enabled = Some(enabled);
        i.client = Some(client.clone());
        i.context = Some(context);
    });
}

/// Records the (hashed) actor of this run.
pub fn note_actor(org: &OrgId, actor: &ActorId) {
    let kind = actor.actor_type().as_str();
    let hash = actor_hash(org, actor);
    with_info(|i| i.actor = Some((kind, hash)));
}

/// Records the peekd auth block, so the event can go over IPC.
pub fn note_auth(auth: &AuthBlock) {
    let auth = auth.clone();
    with_info(|i| i.auth = Some(auth));
}

fn os_version() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let plist =
            std::fs::read_to_string("/System/Library/CoreServices/SystemVersion.plist").ok()?;
        let after = plist.split("<key>ProductVersion</key>").nth(1)?;
        let start = after.find("<string>")? + "<string>".len();
        let end = after[start..].find("</string>")? + start;
        Some(after[start..end].trim().to_owned())
    }
    #[cfg(target_os = "linux")]
    {
        let text = std::fs::read_to_string("/etc/os-release").ok()?;
        text.lines()
            .find_map(|l| l.strip_prefix("VERSION_ID="))
            .map(|v| v.trim_matches('"').to_owned())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

fn locale() -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
        .map(|v| v.split(['.', '@']).next().unwrap_or_default().to_owned())
        .filter(|v| !v.is_empty() && v != "C" && v != "POSIX")
}

/// The `command.finished` record (BLUEPRINT §6.4 envelope).
pub fn command_event(
    command: &str,
    error: Option<&Error>,
    exit_code: u8,
    duration: Duration,
    context: Context,
    actor: Option<&(&'static str, String)>,
) -> Value {
    let environment = if context.is_testing() {
        "testing"
    } else if cfg!(debug_assertions) {
        "development"
    } else {
        "production"
    };
    json!({
        "schema_version": 1,
        "app": "peek",
        "service": "peek-cli",
        "source": "cli",
        "version": silicon_peek_client::VERSION,
        "environment": environment,
        "instance_id": instance_id(),
        "trace_id": trace_id(),
        "step": "command",
        "event": "command.finished",
        "progress": 1,
        "outcome": if error.is_some() { "error" } else { "ok" },
        "duration_ms": u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
        "error_code": error.map(|e| e.code().as_str().to_owned()),
        "exit_code": exit_code,
        "isi": std::env::var("ISI").ok().filter(|v| !v.is_empty() && v.chars().count() <= 160),
        "actor": actor.map(|(kind, hash)| json!({"kind": kind, "hash": hash})),
        "client": {
            "os": std::env::consts::OS,
            "os_version": os_version(),
            "arch": std::env::consts::ARCH,
            "locale": locale(),
        },
        "context": {"command": command, "status": exit_code},
    })
}

/// Sends `command.finished` for this invocation, within the time budget.
pub async fn finish(
    globals: &Globals,
    path: &[String],
    error: Option<&Error>,
    exit_code: u8,
    duration: Duration,
) {
    let Some(first) = path.first() else {
        return;
    };
    if first == "iam" || first.starts_with("__") {
        return;
    }
    let (enabled, client, context, actor, auth) = {
        let mut snapshot = (None, None, None, None, None);
        with_info(|i| {
            snapshot = (
                i.enabled,
                i.client.clone(),
                i.context,
                i.actor.clone(),
                i.auth.clone(),
            );
        });
        snapshot
    };
    let enabled = if let Some(e) = enabled {
        e
    } else {
        // No session was built: honour the home's config if it exists.
        let config = match crate::context::existing_store() {
            Ok(Some(store)) => match store.read_config() {
                Ok(c) => Some(c),
                Err(_) => return,
            },
            Ok(None) => None,
            Err(_) => return,
        };
        globals.telemetry_enabled(config.as_ref())
    };
    if !enabled {
        if silicon_peek_client::telemetry::env_opt_out()
            && let Some(auth) = auth
        {
            forward_env_opt_out(&auth).await;
        }
        return;
    }
    let (client, context) = if let (Some(c), Some(ctx)) = (client, context) {
        (c, ctx)
    } else {
        // A testing run whose session was never resolved: its events
        // must not be tagged production, so drop them.
        if globals.is_testing().unwrap_or(true) {
            return;
        }
        let api = globals
            .explicit_api()
            .ok()
            .flatten()
            .unwrap_or_else(ApiUrl::production);
        match Client::builder(&api)
            .component(concat!("peek-cli/", env!("CARGO_PKG_VERSION")))
            .build()
        {
            Ok(c) => (c.with_trace_id(trace_id()), Context::Production),
            Err(_) => return,
        }
    };
    let record = command_event(
        &path.join(" "),
        error,
        exit_code,
        duration,
        context,
        actor.as_ref(),
    );
    let event = TelemetryEvent {
        id: Uuid::new_v4().hyphenated().to_string(),
        event_type: "command.finished".to_owned(),
        data: record,
        metadata: json!({"occurred_at": Timestamp::now().to_rfc3339(), "source": "cli"}),
    };
    if let Some(auth) = auth
        && via_peekd(&auth, &event).await
    {
        return;
    }
    let batch = TelemetryBatch {
        table: TelemetryTable::Peekclidaemon,
        events: vec![event],
    };
    let _ = tokio::time::timeout(
        POST_TIMEOUT,
        client.telemetry(&batch, "cli", Some(POST_TIMEOUT)),
    )
    .await;
}

#[cfg(target_os = "macos")]
async fn via_peekd(auth: &AuthBlock, event: &TelemetryEvent) -> bool {
    use silicon_peek_client::ipc::cli::Telemetry;
    let task = async {
        let mut service = crate::service::connect_existing(IPC_TIMEOUT).await.ok()??;
        service
            .call(
                &Telemetry {
                    events: vec![event.clone()],
                },
                Some(auth),
                Vec::new(),
                IPC_TIMEOUT,
            )
            .await
            .ok()
    };
    matches!(tokio::time::timeout(IPC_TIMEOUT, task).await, Ok(Some(_)))
}

#[cfg(not(target_os = "macos"))]
#[allow(clippy::unused_async)] // mirrors the macOS signature
async fn via_peekd(_auth: &AuthBlock, _event: &TelemetryEvent) -> bool {
    false
}

/// Tells a running peekd that this home's CLI environment opts out
/// (`config.sync` with `telemetry:false`), so what peekd does for the home
/// is not recorded either. Skipped when the home's config already opts out
/// (peekd reads that itself); never starts peekd; best effort within the
/// IPC budget.
#[cfg(target_os = "macos")]
async fn forward_env_opt_out(auth: &AuthBlock) {
    use silicon_peek_client::{ipc::cli::ConfigSync, runtime::Store};
    let task = async {
        let config = Store::open_existing(std::path::Path::new(&auth.home))
            .ok()?
            .read_config()
            .ok()?;
        if !config.telemetry {
            return None;
        }
        let mut service = crate::service::connect_existing(IPC_TIMEOUT).await.ok()??;
        service
            .call(
                &ConfigSync {
                    config: crate::commands::config::sync_payload(&config),
                },
                Some(auth),
                Vec::new(),
                IPC_TIMEOUT,
            )
            .await
            .ok()
    };
    let _ = tokio::time::timeout(IPC_TIMEOUT, task).await;
}

#[cfg(not(target_os = "macos"))]
#[allow(clippy::unused_async)] // mirrors the macOS signature
async fn forward_env_opt_out(_auth: &AuthBlock) {}

#[cfg(test)]
mod tests {
    use super::*;
    use silicon_peek_client::ErrorCode;

    #[test]
    fn the_event_has_the_envelope_and_no_text() {
        let e = Error::new(ErrorCode::SideTaken, "position 3 is held by si:secret-name");
        let v = command_event(
            "register side",
            Some(&e),
            4,
            Duration::from_millis(412),
            Context::Production,
            Some(&("silicon", "0123456789abcdef".to_owned())),
        );
        assert_eq!(v["event"], "command.finished");
        assert_eq!(v["source"], "cli");
        assert_eq!(v["service"], "peek-cli");
        assert_eq!(v["outcome"], "error");
        assert_eq!(v["error_code"], "side_taken");
        assert_eq!(v["exit_code"], 4);
        assert_eq!(v["duration_ms"], 412);
        assert_eq!(v["context"]["command"], "register side");
        assert_eq!(v["actor"]["hash"], "0123456789abcdef");
        assert!(
            !v.to_string().contains("secret-name"),
            "messages are never recorded"
        );
        let keys: Vec<&String> = v["context"]
            .as_object()
            .map(|m| m.keys().collect())
            .unwrap_or_default();
        for k in keys {
            assert!(silicon_peek_client::telemetry::CONTEXT_KEYS.contains(&k.as_str()));
        }
    }

    #[test]
    fn testing_runs_are_tagged() {
        let v = command_event(
            "send",
            None,
            0,
            Duration::ZERO,
            Context::Testing(Uuid::now_v7()),
            None,
        );
        assert_eq!(v["environment"], "testing");
        assert_eq!(v["outcome"], "ok");
        assert!(v["actor"].is_null());
    }
}
