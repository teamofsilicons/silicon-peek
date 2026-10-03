//! The peekd side of IPC protocol v1 (BLUEPRINT §1.6): accept, check the
//! peer uid (and, for the UI, its executable), handshake with protocol
//! negotiation, then serve CLI requests (one request, one reply; `send
//! --wait` keeps the connection for `ask.result`) or the long-lived UI link.

use std::{
    os::fd::AsRawFd as _,
    sync::Arc,
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use silicon_peek_client::{
    Error, ErrorCode, Result,
    api::TelemetryTable,
    ipc::{
        Empty, Event, Message, Reply, Request, SUPPORTED_PROTOCOLS,
        cli::{
            AppOffer, AppUninstall, AskCancel, AskCancelResult, AskGet, AskList, AskListResult,
            AskResultAck, AskState, Attach, CliHello, ConfigSync, DaemonStatus, DaemonStatusResult,
            Detach, Doctor, Hello, HelloApp, HelloResult, History, HistoryItem, QueueClear,
            QueueList, RegisterDrawing, RegisterSide, ScheduleCancel, ScheduleClear, ScheduleList,
            SendCancel, SendOp, StatusOp, Telemetry, UiStatus, Unregister, Warning, features,
        },
        frame::{AsyncFrameReader, Frame, FrameLimits, blob_limit, write_frame_async},
        negotiate,
        ui::{
            AnswerOp, CancelReason, Dismissed, DrawingError, Focus, MessageOp, Presence,
            SettingsChanged, Shown, ShownDone, SpeechDone, UiAppUninstall, UiDoctor,
            UiStatusReport, UiTelemetry, VoiceSubmit,
        },
    },
    runtime::daemon::verify_peer,
    timestamp::Timestamp,
};
use tokio::{
    net::{UnixListener, UnixStream, unix::OwnedWriteHalf},
    sync::{mpsc, oneshot},
    task::JoinSet,
};

use crate::{
    bubbles::Resolution,
    db::SqlResult as _,
    state::{Shared, SharedRef, WaiterMsg},
    sys,
    telemetry::{Record, check_events, ui_table},
    ui::UiLink,
};

/// How long a new connection may take to say `hello`.
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

/// The CLI ops that need no `auth` block (`app.uninstall` and `doctor`
/// accept one but do not need it).
const NO_AUTH: [&str; 3] = ["daemon.status", "app.uninstall", "doctor"];
/// Ops only a CLI may send.
const CLI_OPS: [&str; 23] = [
    "attach",
    "detach",
    "status",
    "register.side",
    "register.drawing",
    "unregister",
    "send",
    "ask.get",
    "ask.list",
    "ask.cancel",
    "history",
    "config.sync",
    "telemetry",
    "app.offer",
    "daemon.status",
    "app.uninstall",
    "doctor",
    "queue.list",
    "queue.clear",
    "send.cancel",
    "schedule.list",
    "schedule.cancel",
    "schedule.clear",
];
/// Ops only Peek.app may send.
const UI_OPS: [&str; 15] = [
    "answer",
    "voice.submit",
    "ui.status",
    "presence",
    "message",
    "dismissed",
    "speech.done",
    "shown.done",
    "focus",
    "drawing.error",
    "telemetry",
    "settings.changed",
    "permissions.contexts",
    "permissions.ting",
    "shown",
];

/// The first CLI version with queue v2 (`queue_full`, ask state `replaced`).
const QUEUE_V2_CLI: (u64, u64, u64) = (0, 1, 2);

/// Whether a CLI predates 0.1.2 (contract §4.2): it gets `slot_busy` for
/// `queue_full` and ask state `cancelled` for `replaced`. The version is
/// compared as major.minor.patch, ignoring a pre-release; an unparsable one
/// counts as legacy.
fn legacy_cli(hello: &CliHello) -> bool {
    let core = hello
        .cli_version
        .split(['-', '+'])
        .next()
        .unwrap_or_default();
    let parts: Vec<Option<u64>> = core.split('.').map(|p| p.parse().ok()).collect();
    match parts.as_slice() {
        [Some(major), Some(minor), Some(patch)] => (*major, *minor, *patch) < QUEUE_V2_CLI,
        _ => true,
    }
}

/// Rewrites a reply for a legacy CLI (see [`legacy_cli`]).
fn downgrade_reply(op: &str, reply: &mut Reply) {
    match &mut reply.outcome {
        Err(e) if e.code == ErrorCode::QueueFull => e.code = ErrorCode::SlotBusy,
        Err(_) => {}
        Ok(v) => {
            let fix = |state: &mut Value| {
                if state == "replaced" {
                    *state = json!("cancelled");
                }
            };
            match op {
                "ask.get" | "ask.cancel" => {
                    if let Some(st) = v.get_mut("state") {
                        fix(st);
                    }
                }
                "ask.list" => {
                    for a in v
                        .get_mut("asks")
                        .and_then(Value::as_array_mut)
                        .into_iter()
                        .flatten()
                    {
                        if let Some(st) = a.get_mut("state") {
                            fix(st);
                        }
                    }
                }
                "history" => {
                    for i in v
                        .get_mut("items")
                        .and_then(Value::as_array_mut)
                        .into_iter()
                        .flatten()
                    {
                        if let Some(st) = i.get_mut("ask_state") {
                            fix(st);
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

fn protocol_error(msg: impl Into<String>) -> Error {
    Error::new(ErrorCode::ProtocolError, msg).with_hint(
        "the CLI, peekd and Peek.app disagree about the protocol; update with `honeycomb update 'peek'`",
    )
}

async fn write(w: &mut OwnedWriteHalf, m: Message) -> Result<()> {
    let frame = m.into_frame()?;
    write_frame_async(w, &frame, &FrameLimits::V1).await?;
    Ok(())
}

fn ok<T: serde::Serialize>(req: &Request, result: &T, blobs: Vec<Vec<u8>>) -> Reply {
    req.reply(result, blobs)
        .unwrap_or_else(|e| req.reply_err(&e))
}

fn check_blob_limits(req: &Request) -> Result<()> {
    if let Some(limit) = blob_limit(&req.op) {
        for (i, b) in req.blobs.iter().enumerate() {
            if b.len() > limit {
                return Err(Error::new(
                    ErrorCode::FrameTooLarge,
                    format!(
                        "blob {i} of `{}` is {} bytes; the limit is {limit}",
                        req.op,
                        b.len()
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Accepts connections until shutdown, then waits briefly for in-flight
/// requests to finish.
pub async fn serve(shared: SharedRef, listener: UnixListener) {
    let mut shutdown = shared.shutdown.subscribe();
    let mut conns = JoinSet::new();
    loop {
        if *shutdown.borrow() {
            break;
        }
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    let s = Arc::clone(&shared);
                    conns.spawn(async move { s.connection(stream).await });
                }
                Err(e) => {
                    tracing::warn!(error = %e, "accept failed");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            },
            Some(_) = conns.join_next(), if !conns.is_empty() => {}
            _ = shutdown.changed() => break,
        }
    }
    // Finish in-flight writes (§1.8 step 7), then drop the rest.
    let deadline = tokio::time::sleep(Duration::from_secs(2));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            next = conns.join_next() => if next.is_none() { break },
            () = &mut deadline => {
                conns.abort_all();
                break;
            }
        }
    }
}

impl Shared {
    async fn connection(self: SharedRef, stream: UnixStream) {
        if let Err(e) = verify_peer(&stream) {
            tracing::warn!(error = %e, "refused a connection from another user");
            return;
        }
        let peer_pid = sys::peer_pid(stream.as_raw_fd()).ok();
        let (r, mut w) = stream.into_split();
        let mut reader = AsyncFrameReader::new(r);
        let first = match tokio::time::timeout(HELLO_TIMEOUT, reader.read_frame()).await {
            Ok(Ok(Some(f))) => f,
            Ok(Ok(None)) | Err(_) => return,
            Ok(Err(e)) => {
                tracing::debug!(error = %e, "a connection sent an invalid first frame");
                return;
            }
        };
        let req = match Message::from_frame(first) {
            Ok(Message::Request(r)) => r,
            Ok(_) => return,
            Err(e) => {
                tracing::debug!(error = %e, "undecodable first frame");
                return;
            }
        };
        if req.op != "hello" {
            let e = protocol_error(format!("the first frame must be `hello`, not `{}`", req.op));
            let _ = write(&mut w, Message::Reply(req.reply_err(&e))).await;
            return;
        }
        let hello = match req.parse::<Hello>() {
            Ok(h) => h,
            Err(e) => {
                let _ = write(&mut w, Message::Reply(req.reply_err(&e))).await;
                return;
            }
        };
        match hello {
            Hello::Cli(h) => self.cli_hello(&req, h, reader, w).await,
            Hello::Ui(h) => self.ui_hello(&req, h, reader, w, peer_pid).await,
        }
    }

    async fn cli_hello(
        self: SharedRef,
        req: &Request,
        h: silicon_peek_client::ipc::cli::CliHello,
        reader: AsyncFrameReader<tokio::net::unix::OwnedReadHalf>,
        mut w: OwnedWriteHalf,
    ) {
        let Some(protocol) = negotiate(&h.protocols) else {
            let newest_offered = h.protocols.iter().max().copied().unwrap_or(0);
            let ours = SUPPORTED_PROTOCOLS.iter().max().copied().unwrap_or(1);
            let e = if newest_offered > ours {
                if let Some(b) = &h.bundled_app {
                    self.offer_from_hello(b);
                }
                Error::new(
                    ErrorCode::AppUpdatePending,
                    format!(
                        "peekd {} speaks protocol {SUPPORTED_PROTOCOLS:?}; peek {} needs {:?}; Peek.app updates itself",
                        silicon_peek_client::VERSION,
                        h.cli_version,
                        h.protocols
                    ),
                )
                .with_hint("retry in a minute; `peek app update` applies the bundled build")
            } else {
                Error::new(
                    ErrorCode::CliOutdated,
                    format!(
                        "peek {} speaks protocol {:?}; peekd {} needs {SUPPORTED_PROTOCOLS:?}",
                        h.cli_version,
                        h.protocols,
                        silicon_peek_client::VERSION
                    ),
                )
                .with_hint("honeycomb update 'peek' (for a Silicon: SILICON_HOME=<home>/.silicon/packages honeycomb update 'peek')")
                .with_details(json!({
                    "peekd_version": silicon_peek_client::VERSION,
                    "peekd_protocols": SUPPORTED_PROTOCOLS,
                    "cli_protocols": h.protocols,
                }))
            };
            let _ = write(&mut w, Message::Reply(req.reply_err(&e))).await;
            return;
        };
        if let Some(b) = &h.bundled_app {
            self.offer_from_hello(b);
        }
        let result = HelloResult {
            protocol,
            peekd_version: silicon_peek_client::VERSION.to_owned(),
            app: Some(HelloApp {
                build: self.installed_build().await,
                ui_running: self.ui.is_connected(),
            }),
            features: features::ALL.iter().map(|f| (*f).to_owned()).collect(),
        };
        let legacy = legacy_cli(&h);
        if write(&mut w, Message::Reply(ok(req, &result, Vec::new())))
            .await
            .is_ok()
        {
            self.cli_loop(reader, w, legacy).await;
        }
    }

    async fn ui_hello(
        self: SharedRef,
        req: &Request,
        h: silicon_peek_client::ipc::cli::UiHello,
        reader: AsyncFrameReader<tokio::net::unix::OwnedReadHalf>,
        mut w: OwnedWriteHalf,
        peer_pid: Option<i32>,
    ) {
        let exe = peer_pid.and_then(|p| sys::pid_path(p).ok());
        let accepted = exe
            .as_deref()
            .is_some_and(|p| self.cfg.ui_executable.accepts(p));
        if !accepted {
            let e = Error::new(
                ErrorCode::DaemonIdentityMismatch,
                format!(
                    "only Peek.app ({}) may connect as the UI; this peer is {}",
                    self.cfg.ui_executable.describe(),
                    exe.as_deref()
                        .map_or_else(|| "unknown".to_owned(), |p| p.display().to_string())
                ),
            );
            let peer = exe
                .as_deref()
                .map_or_else(|| "unknown".to_owned(), |p| p.display().to_string());
            tracing::warn!(peer = %peer, "refused a UI connection from an unexpected executable");
            let _ = write(&mut w, Message::Reply(req.reply_err(&e))).await;
            return;
        }
        let Some(protocol) = negotiate(&h.protocols) else {
            let e = protocol_error(format!(
                "Peek.app {} speaks protocol {:?}; peekd {} speaks {SUPPORTED_PROTOCOLS:?}",
                h.app_version,
                h.protocols,
                silicon_peek_client::VERSION
            ));
            let _ = write(&mut w, Message::Reply(req.reply_err(&e))).await;
            return;
        };
        let result = HelloResult {
            protocol,
            peekd_version: silicon_peek_client::VERSION.to_owned(),
            app: None,
            features: features::ALL.iter().map(|f| (*f).to_owned()).collect(),
        };
        if write(&mut w, Message::Reply(ok(req, &result, Vec::new())))
            .await
            .is_ok()
        {
            self.ui_session(reader, w, h.app_build, h.app_version, peer_pid)
                .await;
        }
    }

    // ------------------------------------------------------------- CLI

    async fn cli_loop(
        self: SharedRef,
        mut reader: AsyncFrameReader<tokio::net::unix::OwnedReadHalf>,
        mut w: OwnedWriteHalf,
        legacy: bool,
    ) {
        loop {
            let frame = match reader.read_frame().await {
                Ok(Some(f)) => f,
                Ok(None) => return,
                Err(e) => {
                    tracing::debug!(error = %e, "a CLI connection broke");
                    return;
                }
            };
            let req = match Message::from_frame(frame) {
                Ok(Message::Request(r)) => r,
                Ok(_) => {
                    tracing::debug!("a CLI sent a non-request frame");
                    return;
                }
                Err(e) => {
                    tracing::debug!(error = %e, "a CLI sent an undecodable frame");
                    return;
                }
            };
            let started = Instant::now();
            let op = req.op.clone();
            let (mut reply, waiter) = self.cli_request(&req, legacy).await;
            if legacy {
                downgrade_reply(&op, &mut reply);
            }
            let outcome = if reply.outcome.is_ok() { "ok" } else { "error" };
            let error_code = reply.outcome.as_ref().err().map(|e| e.code.to_string());
            if write(&mut w, Message::Reply(reply)).await.is_err() {
                if let Some((ask_id, _)) = waiter {
                    self.drop_waiter(&ask_id).await;
                }
                return;
            }
            let mut rec = Record::new("ipc.request", outcome).with("command", op);
            rec.duration_ms =
                Some(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX));
            rec.error_code = error_code;
            rec.home = req.auth.as_ref().map(|a| a.home.clone());
            self.record(rec);
            if let Some((ask_id, rx)) = waiter {
                self.wait_for_result(&mut reader, &mut w, &ask_id, rx, legacy)
                    .await;
                return;
            }
        }
    }

    async fn drop_waiter(&self, ask_id: &silicon_peek_client::ids::AskId) {
        if let Ok(mut w) = self.waiters.lock() {
            w.remove(ask_id);
        }
        let a = ask_id.as_str().to_owned();
        let _ = self
            .db
            .call(move |c| {
                c.execute("UPDATE asks SET waiter = 0 WHERE ask_id = ?1", [a])
                    .sql()
            })
            .await;
    }

    /// `send --wait`: keeps the connection open until the ask ends (the
    /// result goes to this CLI instead of a ting, D22) or the CLI leaves.
    async fn wait_for_result(
        &self,
        reader: &mut AsyncFrameReader<tokio::net::unix::OwnedReadHalf>,
        w: &mut OwnedWriteHalf,
        ask_id: &silicon_peek_client::ids::AskId,
        rx: oneshot::Receiver<WaiterMsg>,
        legacy: bool,
    ) {
        let mut shutdown = self.shutdown.subscribe();
        if *shutdown.borrow() {
            self.drop_waiter(ask_id).await;
            return;
        }
        tokio::select! {
            msg = rx => {
                if let Ok(WaiterMsg { mut result, ack }) = msg {
                    if legacy && result.state == AskState::Replaced {
                        result.state = AskState::Cancelled;
                    }
                    let written = match Event::new(&result, Vec::new()) {
                        Ok(e) => write(w, Message::Event(e)).await.is_ok(),
                        Err(_) => false,
                    };
                    // A write into the socket buffer is not delivery: the CLI
                    // may be timing out and exiting. Only its ack counts;
                    // otherwise the answer goes by ting (D22).
                    let taken = written && self.await_result_ack(reader, w, ask_id).await;
                    let _ = ack.send(taken);
                }
            }
            _ = reader.read_frame() => {
                // The CLI closed (timeout or ^C) or broke protocol; the
                // answer will go by ting.
                self.drop_waiter(ask_id).await;
            }
            _ = shutdown.changed() => self.drop_waiter(ask_id).await,
        }
    }

    /// Waits up to `waiter_ack` for the CLI's `ask.result.ack` for `ask_id`
    /// (answered with an empty reply, best effort). Anything else (EOF, a
    /// timeout, another frame) means the CLI did not take the result.
    async fn await_result_ack(
        &self,
        reader: &mut AsyncFrameReader<tokio::net::unix::OwnedReadHalf>,
        w: &mut OwnedWriteHalf,
        ask_id: &silicon_peek_client::ids::AskId,
    ) -> bool {
        let frame = tokio::time::timeout(self.cfg.timings.waiter_ack, reader.read_frame()).await;
        let Ok(Ok(Some(frame))) = frame else {
            return false;
        };
        let Ok(Message::Request(req)) = Message::from_frame(frame) else {
            return false;
        };
        let acked = req
            .parse::<AskResultAck>()
            .is_ok_and(|a| &a.ask_id == ask_id);
        if acked && let Ok(reply) = req.reply(&Empty {}, Vec::new()) {
            let _ = write(w, Message::Reply(reply)).await;
        }
        acked
    }

    #[allow(clippy::too_many_lines)] // the CLI op dispatch table, one arm per op
    async fn cli_request(
        self: &SharedRef,
        req: &Request,
        legacy: bool,
    ) -> (
        Reply,
        Option<(
            silicon_peek_client::ids::AskId,
            oneshot::Receiver<WaiterMsg>,
        )>,
    ) {
        if req.op == "hello" {
            return (
                req.reply_err(&protocol_error(
                    "`hello` was already sent on this connection",
                )),
                None,
            );
        }
        if !CLI_OPS.contains(&req.op.as_str()) {
            let msg = if UI_OPS.contains(&req.op.as_str()) {
                format!("op `{}` is only available to Peek.app", req.op)
            } else {
                format!(
                    "peekd {} does not know the op `{}`",
                    silicon_peek_client::VERSION,
                    req.op
                )
            };
            return (
                req.reply_err(
                    &Error::new(ErrorCode::UnknownOp, msg)
                        .with_hint("update peek: honeycomb update 'peek'"),
                ),
                None,
            );
        }
        if let Err(e) = check_blob_limits(req) {
            return (req.reply_err(&e), None);
        }
        if NO_AUTH.contains(&req.op.as_str()) {
            let reply = match req.op.as_str() {
                "app.uninstall" => self.app_uninstall(req).await,
                "doctor" => self.doctor(req).await,
                _ => self.daemon_status(req).await,
            };
            return (reply, None);
        }
        let auth = match req.require_auth() {
            Ok(a) => a.clone(),
            Err(e) => return (req.reply_err(&e), None),
        };
        if req.op == "detach" {
            let r = async {
                req.parse::<Detach>()?;
                self.detach(&auth).await
            }
            .await;
            return (reply_of(req, r), None);
        }
        let caller = match self.authenticate(&auth).await {
            Ok(c) => c,
            Err(e) => return (req.reply_err(&e), None),
        };
        let reply = match req.op.as_str() {
            "attach" => reply_of(
                req,
                async {
                    req.parse::<Attach>()?;
                    self.attach(&caller).await
                }
                .await,
            ),
            "status" => reply_of(
                req,
                async {
                    req.parse::<StatusOp>()?;
                    self.status(&caller).await
                }
                .await,
            ),
            "register.side" => reply_of(
                req,
                async {
                    let op = req.parse::<RegisterSide>()?;
                    self.register_side(&caller, op.index).await
                }
                .await,
            ),
            "register.drawing" => match async {
                let op = req.parse::<RegisterDrawing>()?;
                self.register_drawing(&caller, op, req.blobs.clone()).await
            }
            .await
            {
                Ok((result, blobs)) => ok(req, &result, blobs),
                Err(e) => req.reply_err(&e),
            },
            "unregister" => reply_of(
                req,
                async {
                    req.parse::<Unregister>()?;
                    self.unregister(&caller).await
                }
                .await,
            ),
            "send" => {
                let r = async {
                    let op = req.parse::<SendOp>()?;
                    self.handle_send(&caller, op, req.blobs.clone()).await
                }
                .await;
                return match r {
                    Ok((result, rx)) => {
                        let waiter = rx.and_then(|rx| result.ask_id.clone().map(|a| (a, rx)));
                        (ok(req, &result, Vec::new()), waiter)
                    }
                    Err(e) => (req.reply_err(&e), None),
                };
            }
            "ask.get" => reply_of(
                req,
                async {
                    let op = req.parse::<AskGet>()?;
                    self.get_ask(&caller.key, &op.ask_id).await
                }
                .await,
            ),
            "ask.list" => reply_of(
                req,
                async {
                    let op = req.parse::<AskList>()?;
                    let limit = op.limit.unwrap_or(20);
                    if limit == 0 || limit > 200 {
                        return Err(Error::invalid_input(format!(
                            "--limit {limit} must be 1–200"
                        )));
                    }
                    // A legacy CLI knows `replaced` as `cancelled`.
                    let states = match op.state {
                        Some(AskState::Cancelled) if legacy => {
                            vec![AskState::Cancelled, AskState::Replaced]
                        }
                        Some(s) => vec![s],
                        None => Vec::new(),
                    };
                    Ok(AskListResult {
                        asks: self.list_asks(&caller.key, &states, limit).await?,
                    })
                }
                .await,
            ),
            "ask.cancel" => reply_of(
                req,
                async {
                    let op = req.parse::<AskCancel>()?;
                    let info = self.get_ask(&caller.key, &op.ask_id).await?;
                    let state = if info.state == AskState::Pending {
                        match self
                            .resolve_ask(
                                &op.ask_id,
                                Resolution::Cancelled {
                                    reason: CancelReason::CancelledBySilicon,
                                },
                            )
                            .await
                        {
                            Ok(s) => s,
                            Err(_) => self.get_ask(&caller.key, &op.ask_id).await?.state,
                        }
                    } else {
                        info.state
                    };
                    Ok(AskCancelResult {
                        ask_id: op.ask_id,
                        state,
                    })
                }
                .await,
            ),
            "history" => match async {
                let op = req.parse::<History>()?;
                op.validate()?;
                self.history(&caller.key, op.limit.unwrap_or(50), op.before)
                    .await
            }
            .await
            {
                Ok(items) => ok(req, &json!({ "items": items }), Vec::new()),
                Err(e) => req.reply_err(&e),
            },
            "config.sync" => reply_of(
                req,
                async {
                    let op = req.parse::<ConfigSync>()?;
                    self.config_sync(&caller, &op.config).await?;
                    Ok(Empty {})
                }
                .await,
            ),
            "telemetry" => reply_of(
                req,
                async {
                    let op = req.parse::<Telemetry>()?;
                    check_events(&op.events)?;
                    if self.home_telemetry(&caller.home.home_path).await
                        && !caller.key.context.is_testing()
                    {
                        self.enqueue_telemetry(TelemetryTable::Peekclidaemon, "cli", op.events)
                            .await?;
                    } else if !op.events.is_empty() {
                        // The CLI hands over events only when neither its
                        // home's config nor its environment opts out: an
                        // environment opt-out mirrored earlier is over (this
                        // batch still follows the opt-out it arrived under).
                        self.home_telemetry_confirmed(&caller.home.home_path)
                            .await?;
                    }
                    Ok(Empty {})
                }
                .await,
            ),
            "app.offer" => reply_of(
                req,
                async {
                    let op = req.parse::<AppOffer>()?;
                    self.handle_app_offer(&op).await
                }
                .await,
            ),
            "queue.list" => reply_of(
                req,
                async {
                    req.parse::<QueueList>()?;
                    self.queue_list(&caller).await
                }
                .await,
            ),
            "queue.clear" => reply_of(
                req,
                async {
                    let op = req.parse::<QueueClear>()?;
                    self.queue_clear(&caller, op.all).await
                }
                .await,
            ),
            "send.cancel" => reply_of(
                req,
                async {
                    let op = req.parse::<SendCancel>()?;
                    self.send_cancel(&caller, &op.target).await
                }
                .await,
            ),
            "schedule.list" => reply_of(
                req,
                async {
                    req.parse::<ScheduleList>()?;
                    self.schedule_list(&caller).await
                }
                .await,
            ),
            "schedule.cancel" => reply_of(
                req,
                async {
                    let op = req.parse::<ScheduleCancel>()?;
                    self.schedule_cancel(&caller, &op.target).await
                }
                .await,
            ),
            "schedule.clear" => reply_of(
                req,
                async {
                    req.parse::<ScheduleClear>()?;
                    self.schedule_clear(&caller).await
                }
                .await,
            ),
            other => req.reply_err(&Error::new(
                ErrorCode::UnknownOp,
                format!("op `{other}` is not implemented"),
            )),
        };
        (reply, None)
    }

    async fn daemon_status(&self, req: &Request) -> Reply {
        if let Err(e) = req.parse::<DaemonStatus>() {
            return req.reply_err(&e);
        }
        let ui = self.ui.current();
        let homes: i64 = self
            .db
            .call(|c| {
                c.query_row("SELECT count(*) FROM homes", [], |r| r.get(0))
                    .sql()
            })
            .await
            .unwrap_or(0);
        ok(
            req,
            &DaemonStatusResult {
                running: true,
                pid: std::process::id(),
                version: silicon_peek_client::VERSION.to_owned(),
                protocol: silicon_peek_client::ipc::PROTOCOL,
                socket: self.cfg.socket_path.to_string_lossy().into_owned(),
                ui: UiStatus {
                    running: ui.is_some(),
                    build: ui.map(|l| l.build),
                },
                homes: u32::try_from(homes).unwrap_or(u32::MAX),
            },
            Vec::new(),
        )
    }

    /// `app.uninstall` (from `peek app uninstall`): relayed to Peek.app,
    /// which unregisters its login item and agent, recycles its bundle and
    /// quits. Fails (so the CLI falls back to `open … --args --uninstall`)
    /// when no UI is connected or it does not know the op.
    async fn app_uninstall(&self, req: &Request) -> Reply {
        if let Err(e) = req.parse::<AppUninstall>() {
            return req.reply_err(&e);
        }
        let r = self
            .ui
            .request(
                &UiAppUninstall {},
                Vec::new(),
                self.cfg.timings.ui_request_timeout,
            )
            .await
            .map(|(ui, _)| json!({"accepted": true, "via": "ui", "ui": ui}));
        if r.is_ok() {
            tracing::info!("Peek.app accepted app.uninstall");
        }
        reply_of(req, r)
    }

    /// `doctor` (from `peek doctor`): peekd's own facts plus what only
    /// Peek.app knows (microphone permission, hotkey registration), relayed
    /// with `doctor` to the UI when it is connected. The UI part is `null`
    /// with a `ui_detail` explaining why when it cannot be asked.
    async fn doctor(&self, req: &Request) -> Reply {
        if let Err(e) = req.parse::<Doctor>() {
            return req.reply_err(&e);
        }
        let ui = self.ui.current();
        let mut report = json!({
            "peekd": {
                "version": silicon_peek_client::VERSION,
                "pid": std::process::id(),
                "protocol": silicon_peek_client::ipc::PROTOCOL,
            },
            "ui_running": ui.is_some(),
            "ui_build": ui.as_ref().map(|l| l.build),
            "mic": null,
            "hotkeys": null,
        });
        // What the app last pushed with `ui.status`: the fallback when the
        // live relay below cannot be answered (a busy or older app).
        let pushed = ui
            .as_ref()
            .and_then(|l| l.status())
            .and_then(|s| serde_json::to_value(s).ok());
        if ui.is_none() {
            report["ui_detail"] = json!("Peek.app is not connected to peekd");
        } else {
            let live = self
                .ui
                .request(
                    &UiDoctor {},
                    Vec::new(),
                    self.cfg.timings.ui_request_timeout,
                )
                .await;
            match (live, pushed) {
                (Ok((Value::Object(fields), _)), _) => {
                    for (k, v) in fields {
                        report[k] = v;
                    }
                    report["ui_status_source"] = json!("live");
                }
                (Err(_), Some(Value::Object(fields))) => {
                    for (k, v) in fields {
                        report[k] = v;
                    }
                    report["ui_status_source"] = json!("pushed");
                }
                (Ok((other, _)), _) => report["ui"] = other,
                (Err(e), _) if *e.code() == ErrorCode::UnknownOp => {
                    report["ui_detail"] =
                        json!("this Peek.app does not report microphone and hotkey state yet");
                }
                (Err(e), _) => report["ui_detail"] = json!(e.message()),
            }
        }
        ok(req, &report, Vec::new())
    }

    /// `history`: sends of this Silicon, newest first (plus additive
    /// `warnings`).
    async fn history(
        &self,
        key: &crate::state::ActorKey,
        limit: u32,
        before: Option<String>,
    ) -> Result<Vec<serde_json::Value>> {
        let k = key.clone();
        self.db
            .call(move |c| {
                let mut st = c
                    .prepare(
                        "SELECT s.send_id, s.kind, s.created_at, s.closed_at, s.close_reason, a.ask_id, a.state, s.warnings,
                                s.shown_at, s.expires_at, s.schedule_id, s.due_at
                         FROM sends s LEFT JOIN asks a ON a.send_id = s.send_id
                         WHERE s.context = ?1 AND s.org_id = ?2 AND s.actor_id = ?3 AND (?4 IS NULL OR s.send_id < ?4)
                         ORDER BY s.send_id DESC LIMIT ?5",
                    )
                    .sql()?;
                let rows = st
                    .query_map(
                        rusqlite::params![k.context_str(), k.org.as_str(), k.actor.as_str(), before, limit],
                        |r| {
                            Ok((
                                r.get::<_, String>(0)?,
                                r.get::<_, String>(1)?,
                                r.get::<_, i64>(2)?,
                                r.get::<_, Option<i64>>(3)?,
                                r.get::<_, Option<String>>(4)?,
                                r.get::<_, Option<String>>(5)?,
                                r.get::<_, Option<String>>(6)?,
                                r.get::<_, Option<String>>(7)?,
                                r.get::<_, Option<i64>>(8)?,
                                r.get::<_, Option<i64>>(9)?,
                                r.get::<_, Option<String>>(10)?,
                                r.get::<_, Option<i64>>(11)?,
                            ))
                        },
                    )
                    .sql()?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .sql()?;
                let mut out = Vec::new();
                for (send_id, kind, created, closed, reason, ask_id, ask_state, warnings, shown_at, expires_at, schedule_id, due_at) in rows {
                    let Ok(send_id) = silicon_peek_client::ids::SendId::parse(&send_id) else { continue };
                    let item = HistoryItem {
                        send_id,
                        kind,
                        created_at: Timestamp::from_unix_ms(created),
                        closed_at: closed.map(Timestamp::from_unix_ms),
                        close_reason: reason,
                        ask_id: ask_id.and_then(|a| silicon_peek_client::ids::AskId::parse(&a).ok()),
                        ask_state: ask_state.and_then(|s| serde_json::from_value(serde_json::Value::String(s)).ok()),
                        warnings: warnings
                            .and_then(|w| serde_json::from_str::<Vec<Warning>>(&w).ok())
                            .unwrap_or_default(),
                        shown_at: shown_at.map(Timestamp::from_unix_ms),
                        expires_at: expires_at.map(Timestamp::from_unix_ms),
                        schedule_id: schedule_id
                            .and_then(|s| silicon_peek_client::ids::ScheduleId::parse(&s).ok()),
                        due_at: due_at.map(Timestamp::from_unix_ms),
                    };
                    let v = serde_json::to_value(item)
                        .map_err(|e| Error::internal(format!("serializing history failed: {e}")))?;
                    out.push(v);
                }
                Ok(out)
            })
            .await
    }

    // -------------------------------------------------------------- UI

    async fn ui_session(
        self: SharedRef,
        mut reader: AsyncFrameReader<tokio::net::unix::OwnedReadHalf>,
        mut w: OwnedWriteHalf,
        build: u64,
        version: String,
        pid: Option<i32>,
    ) {
        let (tx, mut rx) = mpsc::unbounded_channel::<Frame>();
        let conn_id = self.ui.next_conn_id();
        let link = Arc::new(UiLink::new(conn_id, tx, build, version.clone(), pid));
        let writer = tokio::spawn(async move {
            while let Some(frame) = rx.recv().await {
                if write_frame_async(&mut w, &frame, &FrameLimits::V1)
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
        if let Some(old) = self.ui.install(Arc::clone(&link)) {
            tracing::info!(
                old = old.conn_id,
                "a new Peek.app connection replaces the old one"
            );
            // Requests waiting on the replaced connection fail now.
            old.close();
            self.on_ui_disconnect().await;
        }
        tracing::info!(build, %version, "Peek.app connected");
        self.record(Record::new("ui.connected", "ok"));
        self.push_slots_state().await;
        self.push_all().await;
        // Anything that came due while Peek.app was away is handled now.
        self.timers_wake.notify_one();
        let mut shutdown = self.shutdown.subscribe();
        loop {
            if *shutdown.borrow() {
                break;
            }
            let frame = tokio::select! {
                f = reader.read_frame() => f,
                _ = shutdown.changed() => break,
            };
            let frame = match frame {
                Ok(Some(f)) => f,
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!(error = %e, "the Peek.app connection broke");
                    break;
                }
            };
            match Message::from_frame(frame) {
                Ok(Message::Request(req)) => {
                    let (reply, stt) = self.ui_request(req, &link).await;
                    let sent = link.send_reply(reply);
                    // After the reply is queued, so its `stt.result` follows
                    // the reply on this connection (never before it).
                    if let Some(job) = stt {
                        job.start(&self);
                    }
                    if !sent {
                        break;
                    }
                }
                Ok(Message::Reply(reply)) => link.complete(reply),
                Ok(Message::Event(e)) => {
                    tracing::debug!(event = %e.event, "ignoring an event from Peek.app");
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Peek.app sent an undecodable frame");
                    break;
                }
            }
        }
        // Every peekd→UI request still waiting on this connection fails at
        // once instead of running to its timeout (90 s for drawing.validate).
        link.close();
        drop(link);
        if self.ui.remove(conn_id) {
            tracing::info!("Peek.app disconnected");
            self.on_ui_disconnect().await;
        }
        writer.abort();
    }

    /// One request from Peek.app: the reply, plus the transcription a
    /// `voice.submit` accepted (started by the caller once the reply is
    /// queued).
    async fn ui_request(
        self: &SharedRef,
        req: Request,
        link: &UiLink,
    ) -> (Reply, Option<crate::stt::SttJob>) {
        if req.op == "voice.submit" {
            if let Err(e) = check_blob_limits(&req) {
                return (req.reply_err(&e), None);
            }
            let submitted = async {
                self.ui_voice_submit(req.parse::<VoiceSubmit>()?, req.blobs.clone())
                    .await
            }
            .await;
            return match submitted {
                Ok((result, job)) => (reply_of(&req, Ok(result)), Some(job)),
                Err(e) => (req.reply_err(&e), None),
            };
        }
        if req.op == "ui.status" {
            // Kept on the connection that sent it: a replaced connection's
            // report never describes the new one.
            let reply = reply_of(
                &req,
                req.parse::<UiStatusReport>().map(|report| {
                    link.set_status(report);
                    Empty {}
                }),
            );
            return (reply, None);
        }
        if req.op == "presence" {
            let reply = match req.parse::<Presence>() {
                Ok(presence) => {
                    self.ui_presence(link, presence).await;
                    reply_of(&req, Ok(Empty {}))
                }
                Err(e) => req.reply_err(&e),
            };
            return (reply, None);
        }
        (self.ui_request_plain(req).await, None)
    }

    /// `presence`: kept on the connection that sent it. When the Carbon comes
    /// back, every bubble held while they were away is shown, in order. Any
    /// change wakes the timers (a display waking from sleep catches up on
    /// due and expired sends at once).
    async fn ui_presence(self: &SharedRef, link: &UiLink, presence: Presence) {
        let before = link.set_presence(presence);
        if before == presence {
            return;
        }
        self.timers_wake.notify_one();
        if before.paused != presence.paused {
            tracing::info!(
                paused = presence.paused,
                "the Carbon {} Peek",
                if presence.paused { "paused" } else { "resumed" }
            );
        }
        let reason = serde_json::to_value(presence.reason)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        if presence.available {
            tracing::info!("the Carbon is back ({reason}); showing held bubbles");
        } else {
            tracing::info!("the Carbon is away ({reason}); new bubbles wait in their queues");
        }
        let current = self.ui.current().is_some_and(|l| l.conn_id == link.conn_id);
        if presence.available && !before.available && current {
            self.push_all().await;
        }
    }

    async fn ui_request_plain(self: &SharedRef, req: Request) -> Reply {
        if !UI_OPS.contains(&req.op.as_str()) {
            let msg = if CLI_OPS.contains(&req.op.as_str()) || req.op == "hello" {
                format!("op `{}` is not available on the UI connection", req.op)
            } else {
                format!(
                    "peekd {} does not know the op `{}`",
                    silicon_peek_client::VERSION,
                    req.op
                )
            };
            return req.reply_err(&Error::new(ErrorCode::UnknownOp, msg));
        }
        if let Err(e) = check_blob_limits(&req) {
            return req.reply_err(&e);
        }
        let empty = |r: Result<()>| reply_of(&req, r.map(|()| Empty {}));
        match req.op.as_str() {
            "permissions.contexts" => reply_of(&req, self.permission_contexts().await),
            "permissions.ting" => reply_of(
                &req,
                async {
                    self.ting_permission(req.parse::<crate::permissions::PermissionAction>()?)
                        .await
                }
                .await,
            ),
            "answer" => empty(async { self.ui_answer(req.parse::<AnswerOp>()?).await }.await),
            "message" => reply_of(
                &req,
                async { self.ui_message(req.parse::<MessageOp>()?).await }.await,
            ),
            "dismissed" => {
                empty(async { self.ui_dismissed(req.parse::<Dismissed>()?).await }.await)
            }
            "speech.done" => {
                empty(async { self.ui_speech_done(req.parse::<SpeechDone>()?).await }.await)
            }
            "shown.done" => {
                empty(async { self.ui_shown_done(req.parse::<ShownDone>()?).await }.await)
            }
            "shown" => empty(async { self.ui_shown(req.parse::<Shown>()?).await }.await),
            "focus" => empty(
                async {
                    let op = req.parse::<Focus>()?;
                    let (key, home) = self.slot_owner_in(op.slot, op.context).await?;
                    self.prewarm(home, key);
                    Ok(())
                }
                .await,
            ),
            "drawing.error" => {
                empty(async { self.ui_drawing_error(req.parse::<DrawingError>()?).await }.await)
            }
            "telemetry" => empty(
                async {
                    let op = req.parse::<UiTelemetry>()?;
                    check_events(&op.events)?;
                    let mut by_table: std::collections::BTreeMap<String, (TelemetryTable, Vec<_>)> =
                        std::collections::BTreeMap::new();
                    for e in op.events {
                        let t = ui_table(&e);
                        by_table
                            .entry(format!("{t:?}"))
                            .or_insert_with(|| (t, Vec::new()))
                            .1
                            .push(e);
                    }
                    for (_, (t, events)) in by_table {
                        self.enqueue_telemetry(t, "mac", events).await?;
                    }
                    Ok(())
                }
                .await,
            ),
            "settings.changed" => empty(
                async {
                    let op = req.parse::<SettingsChanged>()?;
                    let before = self.settings.get();
                    let after = self.settings.apply(&op.key, &op.value)?;
                    if before.telemetry && !after.telemetry {
                        self.clear_telemetry().await?;
                    }
                    if before.show_test_peeks != after.show_test_peeks {
                        self.push_slots_state().await;
                    }
                    Ok(())
                }
                .await,
            ),
            other => req.reply_err(&Error::new(
                ErrorCode::UnknownOp,
                format!("op `{other}` is not implemented"),
            )),
        }
    }
}

fn reply_of<T: serde::Serialize>(req: &Request, r: Result<T>) -> Reply {
    match r {
        Ok(v) => ok(req, &v, Vec::new()),
        Err(e) => req.reply_err(&e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello(version: &str) -> CliHello {
        CliHello {
            cli_version: version.to_owned(),
            protocols: vec![1],
            platform: "macos-aarch64".into(),
            bundled_app: None,
        }
    }

    #[test]
    fn legacy_clis_are_those_before_0_1_2() {
        for (v, legacy) in [
            ("0.1.1", true),
            ("0.1.0", true),
            ("0.0.9", true),
            ("0.1.2", false),
            ("0.1.2-dev.3", false),
            ("0.1.10", false),
            ("0.2.0", false),
            ("1.0.0", false),
            ("0.1.1-rc.1", true),
            ("0.1", true),
            ("garbage", true),
            ("", true),
        ] {
            assert_eq!(legacy_cli(&hello(v)), legacy, "{v}");
        }
    }

    #[test]
    fn legacy_replies_are_downgraded() {
        let e = Error::new(ErrorCode::QueueFull, "full").with_details(json!({"queued": 5}));
        let mut r = Reply::err("1", e.to_object());
        downgrade_reply("send", &mut r);
        let Err(obj) = &r.outcome else {
            panic!("an error reply");
        };
        assert_eq!(obj.code, ErrorCode::SlotBusy);
        assert_eq!(obj.message, "full");
        assert_eq!(obj.details, Some(json!({"queued": 5})));
        let mut r = Reply {
            id: "2".into(),
            outcome: Ok(json!({"asks": [{"state": "replaced"}, {"state": "answered"}]})),
            blobs: Vec::new(),
        };
        downgrade_reply("ask.list", &mut r);
        assert_eq!(
            r.outcome.ok(),
            Some(json!({"asks": [{"state": "cancelled"}, {"state": "answered"}]}))
        );
        for op in ["ask.get", "ask.cancel"] {
            let mut r = Reply {
                id: "3".into(),
                outcome: Ok(json!({"state": "replaced"})),
                blobs: Vec::new(),
            };
            downgrade_reply(op, &mut r);
            assert_eq!(r.outcome.ok(), Some(json!({"state": "cancelled"})));
        }
        let mut r = Reply {
            id: "4".into(),
            outcome: Ok(json!({"items": [{"ask_state": "replaced"}, {"ask_state": null}]})),
            blobs: Vec::new(),
        };
        downgrade_reply("history", &mut r);
        assert_eq!(
            r.outcome.ok(),
            Some(json!({"items": [{"ask_state": "cancelled"}, {"ask_state": null}]}))
        );
    }
}
