//! Shared harness for the `peek` integration tests: a hermetic environment
//! around the real binary (temporary `SILICON_HOME`, private peekd socket,
//! temporary Peek support and Applications directories, launching disabled,
//! telemetry off), session fixtures, and a fake peekd on a Unix socket.
//!
//! Nothing here touches the real user's `~/.peek`, `~/Library/Application
//! Support/Peek`, launchd or any network service: peek-server is wiremock.

#![allow(dead_code, clippy::expect_used, clippy::unwrap_used)]

use std::{
    path::{Path, PathBuf},
    process::Stdio,
};

use serde_json::{Value, json};
use silicon_peek_client::{
    identity::{ApiUrl, Context, SlotKey},
    runtime::{SessionFile, Store},
    timestamp::unix_now,
};
use tokio::io::AsyncWriteExt as _;

pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub const FULL_SCOPE: &str = "self.identity.read self.membership.read self.profile.read";

/// An address nothing listens on (connection refused at once).
pub const DEAD_API: &str = "http://127.0.0.1:9";

/// One `peek` run's result.
#[derive(Debug)]
pub struct Run {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Run {
    /// stdout as exactly one JSON value.
    pub fn json(&self) -> Value {
        serde_json::from_str(self.stdout.trim()).unwrap_or_else(|e| {
            panic!(
                "stdout is not one JSON value ({e}):\n{}\nstderr:\n{}",
                self.stdout, self.stderr
            )
        })
    }

    /// The `{"error":{…}}` object on stderr (the last JSON line).
    pub fn error(&self) -> Value {
        self.stderr
            .lines()
            .rev()
            .find_map(|l| serde_json::from_str::<Value>(l).ok())
            .and_then(|v| v.get("error").cloned())
            .unwrap_or_else(|| panic!("no error object on stderr:\n{}", self.stderr))
    }
}

/// A hermetic environment for the peek binary.
pub struct Env {
    pub dir: tempfile::TempDir,
    pub home: PathBuf,
    pub socket: PathBuf,
    pub support: PathBuf,
    pub apps: PathBuf,
    pub api: String,
    pub vars: Vec<(String, String)>,
}

impl Env {
    /// A new environment whose backend is `api`.
    pub fn new(api: &str) -> Self {
        let dir = tempfile::Builder::new()
            .prefix("pk")
            .tempdir()
            .expect("tempdir");
        let root = dir.path().canonicalize().expect("canonical temp");
        let home = root.join("home");
        std::fs::create_dir(&home).expect("home");
        let support = root.join("support");
        let apps = root.join("apps");
        Self {
            socket: root.join("peekd.sock"),
            home,
            support,
            apps,
            api: api.to_owned(),
            vars: Vec::new(),
            dir,
        }
    }

    /// Adds an environment variable for every run.
    pub fn var(&mut self, k: &str, v: &str) -> &mut Self {
        self.vars.push((k.to_owned(), v.to_owned()));
        self
    }

    /// The per-home store directory.
    pub fn store_dir(&self) -> PathBuf {
        self.home.join(".peek")
    }

    /// Opens (creating) the store.
    pub fn store(&self) -> Store {
        Store::open(&self.store_dir()).expect("store")
    }

    /// The production slot key for this environment's API.
    pub fn slot_key(&self) -> SlotKey {
        SlotKey::new(ApiUrl::parse(&self.api).expect("api"), Context::Production)
    }

    fn command(&self, args: &[&str]) -> tokio::process::Command {
        let mut c = tokio::process::Command::new(env!("CARGO_BIN_EXE_peek"));
        c.args(args)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("SILICON_HOME", &self.home)
            .env("PEEK_API_URL", &self.api)
            .env("PEEK_DAEMON_SOCKET", &self.socket)
            .env("PEEK_INSTALL_SUPPORT_DIR", &self.support)
            .env("PEEK_INSTALL_APPLICATIONS_DIR", &self.apps)
            .env("PEEK_INSTALL_NO_LAUNCH", "1")
            .env("PEEK_TELEMETRY", "0")
            .env("SILICON_HONEYCOMB", self.dir.path().join("no-honeycomb"))
            .current_dir(&self.home)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for (k, v) in &self.vars {
            c.env(k, v);
        }
        c
    }

    /// Runs peek with stdin closed.
    pub async fn run(&self, args: &[&str]) -> Run {
        self.run_stdin(args, None).await
    }

    /// Runs peek, feeding `stdin` (or /dev/null).
    pub async fn run_stdin(&self, args: &[&str], stdin: Option<&[u8]>) -> Run {
        let mut c = self.command(args);
        c.stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        let mut child = c.spawn().expect("spawn peek");
        if let Some(bytes) = stdin {
            let mut pipe = child.stdin.take().expect("stdin pipe");
            pipe.write_all(bytes).await.expect("write stdin");
            drop(pipe);
        }
        let out =
            tokio::time::timeout(std::time::Duration::from_secs(60), child.wait_with_output())
                .await
                .expect("peek finished within 60 s")
                .expect("wait");
        Run {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    /// Writes a logged-in production slot (and a daemon token).
    pub fn login_as(&self, access: &str, refresh: &str, expires_at: i64) -> Store {
        let store = self.store();
        let now = unix_now();
        let slot = json!({
            "actor": {"type": "silicon", "public_id": "si:cleanup"},
            "org_id": "tos", "org_ids": ["tos"], "membership_id": "si:cleanup[tos]",
            "context_id":"080a80f2-248f-4b9f-9a4f-f918a867398d", "scope": FULL_SCOPE, "access_token": access, "refresh_token": refresh,
            "access_expires_at": expires_at, "refresh_started_at": null, "pending_refresh_key": null,
            "logged_in_at": now - 3600, "verified_at": now - 3600,
            "ting": {"subscribed": true, "subscription_id": "sub_1", "registered_at": now - 3600},
            "display_name": "Cleanup", "reconsent_required": false, "rejected": null
        });
        let lock = store.lock().expect("lock");
        let mut file = store.read_session().expect("read");
        file.slots.insert(
            self.slot_key().as_string(),
            serde_json::from_value(slot).expect("slot"),
        );
        store.write_session(&lock, &file).expect("write");
        store.ensure_daemon_token(&lock).expect("token");
        drop(lock);
        store
    }

    /// Reads session.json.
    pub fn session(&self) -> SessionFile {
        self.store().read_session().expect("session")
    }
}

/// A login/refresh response body.
pub fn session_body(access: &str, refresh: &str, expires_in: u64) -> Value {
    json!({
        "access_token": access, "refresh_token": refresh, "token_type": "Bearer",
        "expires_in": expires_in, "scope": FULL_SCOPE,
        "actor": {"type": "silicon", "public_id": "si:cleanup"},
        "org_id": "tos", "org_ids": ["tos"], "membership_id": "si:cleanup[tos]",
        "reconsent_required": false, "display_name": "Cleanup",
        "ting": {"subscribed": true, "subscription_id": "sub_1"},
        "testing_environment": null
    })
}

/// A `GET /api/v1/auth/me` body.
pub fn me_body() -> Value {
    json!({
        "authenticated": true, "actor": {"type": "silicon", "public_id": "si:cleanup"},
        "display_name": "Cleanup", "org_id": "tos", "membership_id": "si:cleanup[tos]",
        "org_role": "member", "scopes": FULL_SCOPE.split(' ').collect::<Vec<_>>(),
        "reconsent_required": false, "ting": {"subscribed": true, "subscription_id": "sub_1"}
    })
}

/// An error body in peek-server's envelope.
pub fn error_body(code: &str, message: &str, retryable: bool) -> Value {
    json!({"error": {"code": code, "message": message, "hint": null,
                     "retryable": retryable, "request_id": "req_1"}})
}

/// A 1×1 PNG header (enough for content sniffing).
pub fn png_bytes() -> Vec<u8> {
    b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec()
}

/// Writes `bytes` at `dir/name` and returns the path.
pub fn write_file(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, bytes).expect("write file");
    p
}

#[cfg(unix)]
pub mod daemon {
    //! A fake peekd: answers `hello`, records every request, and replies with
    //! whatever the test's handler returns.

    use std::sync::{Arc, Mutex};

    use super::*;
    use silicon_peek_client::ipc::{
        Message, Reply, Request,
        cli::{HelloApp, HelloResult},
        frame::{AsyncFrameReader, FrameLimits, write_frame_async},
    };

    pub type Handler = Arc<dyn Fn(&Request) -> Vec<Message> + Send + Sync>;

    /// A recorded request.
    #[derive(Clone, Debug)]
    pub struct Seen {
        pub op: String,
        pub fields: serde_json::Map<String, Value>,
        pub auth: Option<Value>,
        pub blobs: Vec<Vec<u8>>,
    }

    pub struct FakeDaemon {
        pub seen: Arc<Mutex<Vec<Seen>>>,
        task: tokio::task::JoinHandle<()>,
    }

    impl Drop for FakeDaemon {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    impl FakeDaemon {
        /// Every request seen so far, `hello` excluded.
        pub fn seen(&self) -> Vec<Seen> {
            self.seen.lock().unwrap().clone()
        }
    }

    /// Replies `ok` with `result`.
    #[allow(clippy::needless_pass_by_value)] // call sites pass json!() literals
    pub fn ok(request: &Request, result: Value) -> Message {
        Message::Reply(Reply::ok(&request.id, &result, Vec::new()).expect("reply"))
    }

    /// Replies with an error object.
    #[allow(clippy::needless_pass_by_value)] // call sites pass json!() literals
    pub fn err(request: &Request, error: Value) -> Message {
        Message::Reply(Reply::err(
            &request.id,
            serde_json::from_value(error).expect("error object"),
        ))
    }

    /// An event frame.
    pub fn event(name: &str, fields: &Value) -> Message {
        let Some(map) = fields.as_object().cloned() else {
            panic!("event fields must be an object")
        };
        Message::Event(silicon_peek_client::ipc::Event {
            event: name.to_owned(),
            fields: map,
            blobs: Vec::new(),
        })
    }

    /// The hello of a peekd 0.1.1 (no features).
    pub fn legacy_hello() -> HelloResult {
        HelloResult {
            protocol: 1,
            peekd_version: "0.1.0".into(),
            app: Some(HelloApp {
                build: 1000,
                ui_running: true,
            }),
            features: Vec::new(),
        }
    }

    /// The hello of a peekd 0.1.2 (every feature).
    pub fn v2_hello() -> HelloResult {
        HelloResult {
            protocol: 1,
            peekd_version: "0.1.2".into(),
            app: Some(HelloApp {
                build: 1002,
                ui_running: true,
            }),
            features: silicon_peek_client::ipc::cli::features::ALL
                .iter()
                .map(|f| (*f).to_owned())
                .collect(),
        }
    }

    /// Starts the fake on `socket` as an older peekd (no features).
    pub fn start(socket: &Path, handler: Handler) -> FakeDaemon {
        start_with(socket, legacy_hello(), handler)
    }

    /// Starts the fake on `socket` as peekd 0.1.2 (every feature).
    pub fn start_v2(socket: &Path, handler: Handler) -> FakeDaemon {
        start_with(socket, v2_hello(), handler)
    }

    /// Starts the fake on `socket`, answering `hello` with `hello`.
    pub fn start_with(socket: &Path, hello: HelloResult, handler: Handler) -> FakeDaemon {
        let listener = tokio::net::UnixListener::bind(socket).expect("bind fake peekd");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_task = seen.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let handler = handler.clone();
                let seen = seen_task.clone();
                let hello = hello.clone();
                tokio::spawn(async move {
                    let (r, mut w) = stream.into_split();
                    let mut reader = AsyncFrameReader::new(r);
                    while let Ok(Some(frame)) = reader.read_frame().await {
                        let Ok(Message::Request(req)) = Message::from_frame(frame) else {
                            return;
                        };
                        let replies = if req.op == "hello" {
                            vec![Message::Reply(
                                req.reply(&hello, Vec::new()).expect("hello"),
                            )]
                        } else {
                            seen.lock().unwrap().push(Seen {
                                op: req.op.clone(),
                                fields: req.fields.clone(),
                                auth: req
                                    .auth
                                    .as_ref()
                                    .map(|a| serde_json::to_value(a).expect("auth")),
                                blobs: req.blobs.clone(),
                            });
                            handler(&req)
                        };
                        for m in replies {
                            let frame = m.into_frame().expect("frame");
                            if write_frame_async(&mut w, &frame, &FrameLimits::V1)
                                .await
                                .is_err()
                            {
                                return;
                            }
                        }
                    }
                });
            }
        });
        FakeDaemon { seen, task }
    }
}
