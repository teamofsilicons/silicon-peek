//! Shared fixtures: a whole peekd in a temp directory, a wiremock
//! peek-server and a legacy provider, Silicon homes with sessions, a fake CLI and a
//! fake Peek.app speaking IPC v1 over the real socket.

#![allow(
    dead_code,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::missing_panics_doc
)]

use std::{
    collections::{HashMap, VecDeque},
    fmt::Write as _,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use silicon_peek_client::{
    Error, Result, Secret,
    identity::{ApiUrl, Context, SlotKey, TestingSecret},
    ipc::{
        AuthBlock, Event, Message, Op, Reply, Request,
        cli::{Hello, UiHello},
        frame::{AsyncFrameReader, FrameLimits, write_frame_async},
        ui::{DrawingValidateResult, OkResult, ReadyResult},
    },
    runtime::{
        SessionFile, Store,
        daemon::DaemonConnection,
        testing::{SavedEnvironment, TestingFile},
    },
    timestamp::unix_now,
};
use silicon_peek_daemon::{DaemonConfig, DaemonHandle, config::Timings, config::UiExecutableRule};
use tokio::{net::unix::OwnedWriteHalf, sync::oneshot};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, method, path},
};

pub const FULL_SCOPE: &str = "obo:ting:subscriptions.register obo:ting:subscriptions.revoke obo:ting:tings.send self.identity.read self.membership.read self.profile.read";

/// 100 000 bytes of PCM with a recognizable pattern.
pub fn pcm(len: usize) -> Vec<u8> {
    (0..len).map(|i| u8::try_from(i % 251).unwrap()).collect()
}

/// Gemini SSE audio events, intentionally split between PCM samples.
pub fn sse_audio(audio: &[u8]) -> String {
    let mut stream = String::new();
    for chunk in audio.chunks(999) {
        let _ = write!(
            stream,
            "data: {}\n\n",
            json!({
                "event_type": "step.delta", "delta": {"type": "audio", "data": STANDARD.encode(chunk)}
            })
        );
    }
    stream.push_str("data: {\"event_type\":\"interaction.completed\"}\n\ndata: [DONE]\n\n");
    stream
}

pub fn fast_timings() -> Timings {
    let ms = Duration::from_millis;
    Timings {
        outbox_backoff: vec![ms(50), ms(80), ms(120)],
        outbox_steady: ms(200),
        ting_type_missing_retry: Duration::from_mins(15),
        outbox_idle: ms(100),
        authority_sweep: Duration::from_secs(600),
        tts_retry: vec![ms(20), ms(50)],
        tts_first_audio_budget: Duration::from_secs(3),
        tts_idle: Duration::from_secs(3),
        stt_retry: vec![ms(20), ms(50)],
        stt_budget: Duration::from_secs(5),
        refresh_retry: vec![],
        ui_connect_wait: Duration::from_secs(2),
        ui_request_timeout: Duration::from_secs(3),
        drawing_validate_timeout: Duration::from_secs(5),
        telemetry_interval: Duration::from_secs(3600),
        update_interval: Duration::from_secs(3600),
        update_initial_delay: Duration::from_secs(3600),
        update_defer_poll: ms(50),
        update_defer_max: ms(200),
        ui_quit_wait: ms(300),
        watchdog_behind: Duration::from_secs(7200),
        launch_grace: ms(10),
        bubble_grace: Duration::from_secs(60),
        waiter_ack: Duration::from_secs(2),
        prewarm_cooldown: Duration::from_secs(60),
        timer_catchup_cap: Duration::from_secs(15),
    }
}

pub struct Harness {
    pub root: tempfile::TempDir,
    pub cfg: DaemonConfig,
    pub handle: Option<DaemonHandle>,
    pub server: MockServer,
    pub deepgram: MockServer,
}

pub fn this_exe() -> PathBuf {
    std::env::current_exe().unwrap().canonicalize().unwrap()
}

impl Harness {
    /// A daemon with test timings; `tweak` edits the config first.
    pub async fn start_with(tweak: impl FnOnce(&mut DaemonConfig)) -> Self {
        Self::start_with_server(|c, _| tweak(c)).await
    }

    /// As [`Harness::start_with`], with the mock peek-server's URL.
    pub async fn start_with_server(tweak: impl FnOnce(&mut DaemonConfig, &str)) -> Self {
        let root = tempfile::tempdir().unwrap();
        let mut cfg = DaemonConfig::rooted(root.path()).unwrap();
        cfg.timings = fast_timings();
        cfg.ui_executable = UiExecutableRule::Exact(this_exe());
        let server = MockServer::start().await;
        let deepgram = MockServer::start().await;
        tweak(&mut cfg, &server.uri());
        let handle = silicon_peek_daemon::start(cfg.clone()).await.unwrap();
        let h = Self {
            root,
            cfg,
            handle: Some(handle),
            server,
            deepgram,
        };
        h.mount_defaults().await;
        h
    }

    pub async fn start() -> Self {
        Self::start_with(|_| {}).await
    }

    pub fn handle(&self) -> &DaemonHandle {
        self.handle.as_ref().unwrap()
    }

    /// Stops and restarts the daemon on the same directories.
    pub async fn restart(&mut self) {
        let h = self.handle.take().unwrap();
        h.shutdown().await;
        self.handle = Some(silicon_peek_daemon::start(self.cfg.clone()).await.unwrap());
    }

    pub fn api(&self) -> ApiUrl {
        ApiUrl::parse(&self.server.uri()).unwrap()
    }

    pub fn db(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.cfg.support_dir.join("peekd.sqlite")).unwrap()
    }

    /// Gemini TTS, `OpenAI` STT and drawing sync defaults.
    pub async fn mount_defaults(&self) {
        Mock::given(method("POST"))
            .and(path("/api/v1/speech/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "provider": "openai", "mode": "proxy", "expires_in": 600,
                "base_url": format!("{}/api/v1/speech", self.server.uri()),
                "key_source": "peek", "params": {"mip_opt_out": true, "tags": []}
            })))
            .mount(&self.server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/speech/token"))
            .and(body_json(json!({"purpose": "tts"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "provider": "gemini", "mode": "proxy", "expires_in": 3600,
                "base_url": format!("{}/api/v1/speech", self.server.uri()),
                "key_source": "peek", "params": {"mip_opt_out": true, "tags": []}
            })))
            .with_priority(2)
            .mount(&self.server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/api/v1/drawings/current"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha256": "0".repeat(64), "bytes": 1, "updated_at": "2026-09-26T10:00:00Z"
            })))
            .mount(&self.server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/drawings/current"))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({
                "error": {"code": "drawing_not_found", "message": "none", "retryable": false}
            })))
            .mount(&self.server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/api/v1/drawings/current"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&self.server)
            .await;
    }

    /// Mounts an accepting deliveries endpoint.
    pub async fn accept_deliveries(&self) {
        Mock::given(method("POST"))
            .and(path("/api/v1/deliveries"))
            .respond_with(|req: &wiremock::Request| {
                let v: Value = serde_json::from_slice(&req.body).unwrap_or_default();
                ResponseTemplate::new(200).set_body_json(json!({
                    "event_id": v["event_id"], "ting_id": format!("msg_{}", &v["event_id"].as_str().unwrap_or("x")[4..]),
                    "status": "accepted", "silent": false, "replayed": false
                }))
            })
            .mount(&self.server)
            .await;
    }

    /// A Silicon home logged in to the mock server as `actor`.
    pub fn home(&self, actor: &str) -> Home {
        Home::create(self.root.path(), &self.api(), actor)
    }

    /// A CLI connection after `hello`.
    pub async fn cli(&self) -> DaemonConnection {
        let mut c = DaemonConnection::connect(self.handle().socket_path(), Duration::from_secs(2))
            .await
            .unwrap();
        c.hello(&Hello::cli("macos-aarch64", None), Duration::from_secs(5))
            .await
            .unwrap();
        c
    }

    /// A CLI connection whose hello claims `version` (a legacy CLI for
    /// `0.1.1`).
    pub async fn cli_as(&self, version: &str) -> DaemonConnection {
        let mut c = DaemonConnection::connect(self.handle().socket_path(), Duration::from_secs(2))
            .await
            .unwrap();
        let hello = Hello::Cli(silicon_peek_client::ipc::cli::CliHello {
            cli_version: version.to_owned(),
            protocols: vec![1],
            platform: "macos-aarch64".into(),
            bundled_app: None,
        });
        c.hello(&hello, Duration::from_secs(5)).await.unwrap();
        c
    }

    /// One CLI op for `home`.
    pub async fn call<O: Op>(
        &self,
        home: &Home,
        op: &O,
        blobs: Vec<Vec<u8>>,
    ) -> Result<(O::Output, Vec<Vec<u8>>)> {
        let mut c = self.cli().await;
        c.call(op, Some(&home.auth), blobs, Duration::from_secs(20))
            .await
    }

    pub async fn ui(&self) -> FakeUi {
        FakeUi::connect(self.handle().socket_path()).await
    }

    /// A fake Peek.app of `build` (1002 and up report `shown`).
    pub async fn ui_build(&self, build: u64) -> FakeUi {
        FakeUi::try_connect_build(self.handle().socket_path(), build)
            .await
            .unwrap()
    }

    /// Registers a side and a drawing (through the fake UI) for `home`.
    pub async fn register(&self, home: &Home, side: u64, ui: &FakeUi) {
        use silicon_peek_client::{
            identity::SlotIndex,
            ipc::cli::{RegisterDrawing, RegisterSide},
        };
        self.call(
            home,
            &RegisterSide {
                index: SlotIndex::new(side).unwrap(),
            },
            vec![],
        )
        .await
        .unwrap();
        let _ = ui;
        self.call(
            home,
            &RegisterDrawing {
                filename: "logo.js".into(),
                check_only: false,
                preview: false,
                dump_frame: None,
            },
            vec![DRAWING.to_vec()],
        )
        .await
        .unwrap();
    }
}

pub const DRAWING: &[u8] = b"export default function draw(ctx, input) { ctx.fillStyle = 'white'; ctx.fillRect(0,0,100,100); }\n";

pub struct Home {
    pub dir: PathBuf,
    pub store: Store,
    pub auth: AuthBlock,
    pub actor: String,
}

impl Home {
    pub fn create(root: &Path, api: &ApiUrl, actor: &str) -> Self {
        let silicon_home = root.join(format!("homes/{}", actor.replace(':', "_")));
        std::fs::create_dir_all(&silicon_home).unwrap();
        let store = Store::open(&silicon_home.join(".peek")).unwrap();
        let lock = store.lock().unwrap();
        let token = store.ensure_daemon_token(&lock).unwrap();
        let key = SlotKey::new(api.clone(), Context::Production);
        let now = unix_now();
        let slot = json!({
            "actor": {"type": "silicon", "public_id": actor},
            "org_id": "tos", "org_ids": ["tos"], "membership_id": format!("{actor}[tos]"),
            "scope": FULL_SCOPE, "access_token": format!("oat_{}", actor.replace(':', "")),
            "refresh_token": format!("ort_{}", actor.replace(':', "")),
            "access_expires_at": now + 3600, "refresh_started_at": null, "pending_refresh_key": null,
            "logged_in_at": now - 60, "verified_at": now - 60,
            "ting": {"subscribed": true, "subscription_id": "sub_1", "registered_at": now - 60},
            "display_name": "Cleanup Bot", "reconsent_required": false, "rejected": null
        });
        let mut file = SessionFile::default();
        file.slots
            .insert(key.as_string(), serde_json::from_value(slot).unwrap());
        store.write_session(&lock, &file).unwrap();
        drop(lock);
        let auth = AuthBlock {
            home: store.dir().to_string_lossy().into_owned(),
            home_token: token,
            api_url: api.clone(),
            context: Context::Production,
        };
        Self {
            dir: silicon_home,
            store,
            auth,
            actor: actor.to_owned(),
        }
    }

    /// A home logged in to testing environment `env` (saved in testing.json
    /// with `secret` and `generation`), instead of production.
    pub fn create_testing(
        root: &Path,
        api: &ApiUrl,
        actor: &str,
        env: uuid::Uuid,
        secret: &str,
        generation: u64,
    ) -> Self {
        let mut home = Self::create(root, api, actor);
        let lock = home.store.lock().unwrap();
        let mut file = home.store.read_session().unwrap();
        let production = SlotKey::new(api.clone(), Context::Production).as_string();
        let slot = file.slots.remove(&production).unwrap();
        file.slots.insert(
            SlotKey::new(api.clone(), Context::Testing(env)).as_string(),
            slot,
        );
        home.store.write_session(&lock, &file).unwrap();
        let mut testing = TestingFile::default();
        testing.save(
            env,
            SavedEnvironment {
                api_url: api.clone(),
                app_secret: TestingSecret::parse(secret).unwrap(),
                name: "peek testing".into(),
                generation,
                extra: std::collections::BTreeMap::new(),
            },
        );
        home.store.write_testing(&lock, &testing).unwrap();
        drop(lock);
        home.auth.context = Context::Testing(env);
        home
    }

    /// Edits `session.json`.
    pub fn edit_session(&self, f: impl FnOnce(&mut SessionFile)) {
        let lock = self.store.lock().unwrap();
        let mut s = self.store.read_session().unwrap();
        f(&mut s);
        self.store.write_session(&lock, &s).unwrap();
    }

    pub fn chmod(&self, mode: u32) {
        std::fs::set_permissions(self.store.dir(), std::fs::Permissions::from_mode(mode)).unwrap();
    }

    pub fn with_token(&self, token: &str) -> AuthBlock {
        let mut a = self.auth.clone();
        a.home_token = Secret::new(token);
        a
    }
}

/// How the fake UI answers `drawing.validate`.
#[derive(Clone, Debug)]
pub enum Validate {
    Ok,
    Fail(String),
}

type Pending = Arc<Mutex<HashMap<String, oneshot::Sender<Reply>>>>;

/// A fake Peek.app: records events, answers peekd's requests.
pub struct FakeUi {
    writer: Arc<tokio::sync::Mutex<OwnedWriteHalf>>,
    events: Arc<Mutex<VecDeque<Event>>>,
    notify: Arc<tokio::sync::Notify>,
    pending: Pending,
    pub validate: Arc<Mutex<Validate>>,
    pub requests: Arc<Mutex<Vec<(String, Value)>>>,
    pub validated_scripts: Arc<Mutex<Vec<Vec<u8>>>>,
    pub ready: Arc<Mutex<bool>>,
    /// Overrides `ready` for `app.quit` only (a UI that turned busy between
    /// `app.update.prepare` and `app.quit`).
    pub quit_ready: Arc<Mutex<Option<bool>>>,
    /// Whether the fake answers peekd's live `doctor` relay (false: an app
    /// that answers `unknown_op`, so peekd falls back to `ui.status`).
    pub answers_doctor: Arc<Mutex<bool>>,
    /// Replies and events in the order they arrived (`reply` or
    /// `event:<name>`).
    pub arrivals: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

async fn send(writer: &tokio::sync::Mutex<OwnedWriteHalf>, m: Message) {
    let frame = m.into_frame().unwrap();
    let mut w = writer.lock().await;
    write_frame_async(&mut *w, &frame, &FrameLimits::V1)
        .await
        .unwrap();
}

/// The fake app's answer to one peekd→UI request.
fn fake_reply(
    req: &Request,
    validate: &Mutex<Validate>,
    scripts: &Mutex<Vec<Vec<u8>>>,
    ready: &Mutex<bool>,
    quit_ready: &Mutex<Option<bool>>,
    answers_doctor: &Mutex<bool>,
) -> Reply {
    match req.op.as_str() {
        "doctor" if !*answers_doctor.lock().unwrap() => req.reply_err(&Error::new(
            silicon_peek_client::ErrorCode::UnknownOp,
            "fake UI does not answer doctor",
        )),
        "drawing.validate" => {
            let path = req.fields["script_path"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            scripts
                .lock()
                .unwrap()
                .push(std::fs::read(&path).unwrap_or_default());
            let mode = validate.lock().unwrap().clone();
            let stats = json!({"frames":90,"p50_ms":0.31,"p95_ms":0.58,"max_ms":0.92,"ops_max":212,"glass_rebuilds":1});
            let body: DrawingValidateResult = match mode {
                Validate::Ok => serde_json::from_value(json!({
                    "ok": true, "stats": stats, "warnings": [], "logs": ["hello from the drawing"], "error": null
                }))
                .unwrap(),
                Validate::Fail(m) => serde_json::from_value(json!({
                    "ok": false, "stats": stats, "warnings": [], "logs": [],
                    "error": {"message": m, "stack": "at draw (logo.js:1:10)", "frame": 12, "input_summary": {"phase":"idle"}}
                }))
                .unwrap(),
            };
            let blobs = if req.fields["preview"] == json!(true) {
                vec![b"\x89PNG\r\n\x1a\npreview".to_vec()]
            } else {
                vec![]
            };
            req.reply(&body, blobs).unwrap()
        }
        "drawing.load" => req.reply(&OkResult { ok: true }, vec![]).unwrap(),
        "doctor" => req
            .reply(
                &json!({"mic": "granted", "hotkeys": {"registered": ["ctrl+cmd+3"], "failed": []}}),
                vec![],
            )
            .unwrap(),
        "app.uninstall" => req.reply(&json!({"accepted": true}), vec![]).unwrap(),
        "app.update.prepare" => {
            let r = *ready.lock().unwrap();
            req.reply(&ReadyResult { ready: r }, vec![]).unwrap()
        }
        "app.quit" => {
            let r = quit_ready
                .lock()
                .unwrap()
                .unwrap_or_else(|| *ready.lock().unwrap());
            req.reply(&ReadyResult { ready: r }, vec![]).unwrap()
        }
        other => req.reply_err(&Error::new(
            silicon_peek_client::ErrorCode::UnknownOp,
            format!("fake UI does not handle {other}"),
        )),
    }
}

/// A CLI connection of a given version, for legacy-downgrade tests.
pub struct DaemonCli {
    conn: DaemonConnection,
}

impl DaemonCli {
    /// Connects as a CLI of `version`.
    pub async fn legacy(h: &Harness, version: &str) -> Self {
        Self {
            conn: h.cli_as(version).await,
        }
    }

    /// One op for `home` on this connection.
    pub async fn call<O: Op>(&mut self, home: &Home, op: &O) -> Result<O::Output> {
        self.conn
            .call(op, Some(&home.auth), vec![], Duration::from_secs(20))
            .await
            .map(|(o, _)| o)
    }

    /// The connection itself (for `--wait` events).
    pub fn conn(&mut self) -> &mut DaemonConnection {
        &mut self.conn
    }
}

impl FakeUi {
    pub async fn try_connect(socket: &Path) -> Result<Self> {
        Self::try_connect_build(socket, 1000).await
    }

    pub async fn try_connect_build(socket: &Path, build: u64) -> Result<Self> {
        let stream = tokio::net::UnixStream::connect(socket)
            .await
            .map_err(|e| Error::internal(e.to_string()))?;
        let (r, w) = stream.into_split();
        let writer = Arc::new(tokio::sync::Mutex::new(w));
        let mut reader = AsyncFrameReader::new(r);
        let hello = Request::new(
            &Hello::Ui(UiHello {
                app_build: build,
                app_version: if build >= 1002 { "0.1.2" } else { "0.1.0" }.into(),
                protocols: vec![1],
            }),
            None,
            vec![],
        )?;
        send(&writer, Message::Request(hello)).await;
        let frame = reader
            .read_frame()
            .await?
            .ok_or_else(|| Error::internal("closed"))?;
        let Message::Reply(reply) = Message::from_frame(frame)? else {
            return Err(Error::internal("expected a hello reply"));
        };
        reply.into_result::<Value>()?;
        let events = Arc::new(Mutex::new(VecDeque::new()));
        let notify = Arc::new(tokio::sync::Notify::new());
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let validate = Arc::new(Mutex::new(Validate::Ok));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let validated_scripts = Arc::new(Mutex::new(Vec::new()));
        let ready = Arc::new(Mutex::new(true));
        let quit_ready = Arc::new(Mutex::new(None));
        let answers_doctor = Arc::new(Mutex::new(true));
        let arrivals = Arc::new(Mutex::new(Vec::new()));
        let task = {
            let (events, notify, pending, validate, requests, scripts, ready, quit, writer) = (
                Arc::clone(&events),
                Arc::clone(&notify),
                Arc::clone(&pending),
                Arc::clone(&validate),
                Arc::clone(&requests),
                Arc::clone(&validated_scripts),
                Arc::clone(&ready),
                Arc::clone(&quit_ready),
                Arc::clone(&writer),
            );
            let (doctor, arrived) = (Arc::clone(&answers_doctor), Arc::clone(&arrivals));
            tokio::spawn(async move {
                while let Ok(Some(frame)) = reader.read_frame().await {
                    match Message::from_frame(frame) {
                        Ok(Message::Event(e)) => {
                            arrived.lock().unwrap().push(format!("event:{}", e.event));
                            events.lock().unwrap().push_back(e);
                            notify.notify_waiters();
                        }
                        Ok(Message::Reply(r)) => {
                            arrived.lock().unwrap().push("reply".to_owned());
                            if let Some(tx) = pending.lock().unwrap().remove(&r.id) {
                                let _ = tx.send(r);
                            }
                        }
                        Ok(Message::Request(req)) => {
                            requests
                                .lock()
                                .unwrap()
                                .push((req.op.clone(), Value::Object(req.fields.clone())));
                            let reply =
                                fake_reply(&req, &validate, &scripts, &ready, &quit, &doctor);
                            send(&writer, Message::Reply(reply)).await;
                        }
                        Err(_) => break,
                    }
                }
            })
        };
        Ok(Self {
            writer,
            events,
            notify,
            pending,
            validate,
            requests,
            validated_scripts,
            ready,
            quit_ready,
            answers_doctor,
            arrivals,
            task,
        })
    }

    pub async fn connect(socket: &Path) -> Self {
        Self::try_connect(socket).await.unwrap()
    }

    /// Sends a UI→peekd request and waits for the reply.
    pub async fn request<O: Op>(&self, op: &O, blobs: Vec<Vec<u8>>) -> Result<O::Output> {
        let req = Request::new(op, None, blobs)?;
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(req.id.clone(), tx);
        send(&self.writer, Message::Request(req)).await;
        let reply = tokio::time::timeout(Duration::from_secs(10), rx)
            .await
            .map_err(|_| Error::internal("no reply"))?
            .map_err(|_| Error::internal("closed"))?;
        reply.into_result::<O::Output>().map(|(o, _)| o)
    }

    /// The next event named `name` (earlier events of other names are kept).
    pub async fn expect(&self, name: &str) -> Event {
        self.try_expect(name, Duration::from_secs(10))
            .await
            .unwrap_or_else(|| panic!("no `{name}` event arrived; got {:?}", self.names()))
    }

    pub async fn try_expect(&self, name: &str, timeout: Duration) -> Option<Event> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let notified = self.notify.notified();
            {
                let mut q = self.events.lock().unwrap();
                if let Some(i) = q.iter().position(|e| e.event == name) {
                    return q.remove(i);
                }
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return None;
            }
        }
    }

    /// Names of buffered events.
    pub fn names(&self) -> Vec<String> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .map(|e| e.event.clone())
            .collect()
    }

    /// Drains buffered events.
    pub fn drain(&self) -> Vec<Event> {
        self.events.lock().unwrap().drain(..).collect()
    }

    /// Collects the whole TTS stream of a send.
    pub async fn tts_stream(&self, send_id: &str) -> (Value, Vec<u8>, u64) {
        let begin = self.expect("tts.begin").await;
        assert_eq!(begin.fields["send_id"], send_id);
        let mut bytes = Vec::new();
        let mut seq = 0;
        loop {
            let deadline = Duration::from_secs(10);
            let chunk = self
                .try_expect("tts.chunk", Duration::from_millis(200))
                .await;
            if let Some(c) = chunk {
                assert_eq!(c.fields["seq"], seq, "chunks arrive in order");
                assert_eq!(c.blobs.len(), 1);
                assert!(
                    c.blobs[0].len() <= 64 * 1024,
                    "a tts.chunk is at most 64 KiB"
                );
                assert_eq!(c.blobs[0].len() % 2, 0, "chunks carry whole samples");
                bytes.extend_from_slice(&c.blobs[0]);
                seq += 1;
                continue;
            }
            let end = self.try_expect("tts.end", deadline).await.expect("tts.end");
            let total = end.fields["total_frames"].as_u64().unwrap();
            return (begin.fields.clone().into_iter().collect(), bytes, total);
        }
    }

    pub fn close(self) {
        self.task.abort();
    }
}

/// Waits until `cond` holds (polling), or panics after `secs`.
pub async fn eventually(secs: u64, what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    while tokio::time::Instant::now() < deadline {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for {what}");
}

/// One column of one row.
pub fn query_one<T: rusqlite::types::FromSql>(
    db: &rusqlite::Connection,
    sql: &str,
    p: &[&dyn rusqlite::ToSql],
) -> Option<T> {
    db.query_row(sql, p, |r| r.get(0)).ok()
}
