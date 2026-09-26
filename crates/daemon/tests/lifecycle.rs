//! Start/stop rules (§1.8), peekd's use of `fresh_session` before a delivery
//! (§2.5 margins, §3.6 401 handling), and the `peekd` binary itself.

#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use std::{os::unix::fs::PermissionsExt as _, process::Stdio, time::Duration};

use common::{Harness, eventually, query_one};
use serde_json::json;
use silicon_peek_client::{
    ErrorCode,
    ipc::{
        cli::{DaemonStatus, Hello, SendOp},
        ui::{AnswerOp, UiAnswerVia},
    },
    runtime::daemon::DaemonConnection,
    schema::ask::Ask,
    timestamp::unix_now,
};
use wiremock::{
    Mock, ResponseTemplate,
    matchers::{body_json, header, method, path},
};

fn ask_op() -> SendOp {
    SendOp {
        isi: None,
        speak: None,
        show: None,
        ask: Some(
            Ask::from_input(
                &json!({"question":"Ship it?","type":"single_choice","options":["Yes","No"]}),
            )
            .unwrap(),
        ),
        voice: None,
        lang: None,
        notify: vec![],
        duration_ms: None,
        expires_in_s: None,
        wait: false,
    }
}

fn session(access: &str, refresh: &str) -> serde_json::Value {
    json!({
        "access_token": access, "refresh_token": refresh, "token_type": "Bearer", "expires_in": 1800,
        "scope": common::FULL_SCOPE, "actor": {"type":"silicon","public_id":"si:cleanup"},
        "org_id": "tos", "org_ids": ["tos"], "membership_id": "si:cleanup[tos]",
        "reconsent_required": false, "display_name": "Cleanup Bot"
    })
}

#[tokio::test]
async fn one_peekd_per_account_and_a_clean_shutdown() {
    let h = Harness::start().await;
    let e = silicon_peek_daemon::start(h.cfg.clone())
        .await
        .err()
        .unwrap();
    assert_eq!(*e.code(), ErrorCode::DaemonRunning);
    let mode = std::fs::metadata(h.handle().socket_path())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    let dir_mode = std::fs::metadata(h.handle().socket_path().parent().unwrap())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(dir_mode & 0o777, 0o700);
    let db_mode = std::fs::metadata(h.cfg.support_dir.join("peekd.sqlite"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(db_mode & 0o077, 0, "no tokens, and private anyway");

    let mut h = h;
    let socket = h.handle().socket_path().to_path_buf();
    let code = h.handle.take().unwrap().shutdown().await;
    assert_eq!(code, 0);
    assert!(!socket.exists(), "the socket is removed on shutdown");
    // The lock is released: a new daemon starts, replacing a stale socket.
    std::fs::write(&socket, b"stale").unwrap();
    let again = silicon_peek_daemon::start(h.cfg.clone()).await.unwrap();
    let mut c = DaemonConnection::connect(&socket, Duration::from_secs(2))
        .await
        .unwrap();
    c.hello(&Hello::cli("macos-aarch64", None), Duration::from_secs(2))
        .await
        .unwrap();
    again.shutdown().await;
}

#[tokio::test]
async fn deliveries_refresh_near_expiry_and_retry_once_after_a_401() {
    let h = Harness::start().await;
    let ui = h.ui().await;
    let home = h.home("si:cleanup");
    h.register(&home, 1, &ui).await;
    // The access token expires within the 120 s delivery margin.
    home.edit_session(|s| {
        for slot in s.slots.values_mut() {
            slot.access_expires_at = unix_now() + 30;
        }
    });
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/refresh"))
        .and(body_json(json!({"refresh_token": "ort_sicleanup"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(session("oat_second", "ort_second")))
        .expect(1)
        .mount(&h.server)
        .await;
    // The backend says the new token is not active (IAM 401) once …
    Mock::given(method("POST"))
        .and(path("/api/v1/deliveries"))
        .and(header("authorization", "Bearer oat_second"))
        .respond_with(ResponseTemplate::new(401).set_body_json(
            json!({"error":{"code":"unauthenticated","message":"inactive","retryable":false}}),
        ))
        .expect(1)
        .mount(&h.server)
        .await;
    // … so peekd forces one rotation and retries with the newer token.
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/refresh"))
        .and(body_json(json!({"refresh_token": "ort_second"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(session("oat_third", "ort_third")))
        .expect(1)
        .mount(&h.server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/deliveries"))
        .and(header("authorization", "Bearer oat_third"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "event_id": "evt_x", "ting_id": "msg_1", "status": "accepted", "silent": true, "replayed": false
        })))
        .expect(1)
        .mount(&h.server)
        .await;
    let (r, _) = h.call(&home, &ask_op(), vec![]).await.unwrap();
    let _ = ui.expect("peek.show").await;
    let ask_id = r.ask_id.unwrap();
    ui.request(
        &AnswerOp {
            send_id: r.send_id,
            ask_id: ask_id.clone(),
            value: json!("1"),
            via: UiAnswerVia::Click,
        },
        vec![],
    )
    .await
    .unwrap();
    let db = h.db();
    eventually(10, "the delivery", || {
        query_one::<String>(
            &db,
            "SELECT status FROM outbox WHERE subject_id = ?1",
            &[&ask_id.as_str()],
        )
        .as_deref()
            == Some("accepted")
    })
    .await;
    let silent: i64 = query_one(
        &db,
        "SELECT silent FROM outbox WHERE subject_id = ?1",
        &[&ask_id.as_str()],
    )
    .unwrap();
    assert_eq!(silent, 1, "muted by the Silicon, recorded for the history");
    let slot = home
        .store
        .read_session()
        .unwrap()
        .slots
        .into_values()
        .next()
        .unwrap();
    assert_eq!(
        slot.refresh_token.expose(),
        "ort_third",
        "peekd wrote the rotation back to the home's store"
    );
}

#[tokio::test]
async fn a_rejected_session_parks_rows_until_the_next_login() {
    let h = Harness::start().await;
    let ui = h.ui().await;
    let home = h.home("si:cleanup");
    h.register(&home, 1, &ui).await;
    home.edit_session(|s| {
        for slot in s.slots.values_mut() {
            slot.access_expires_at = unix_now() + 30;
        }
    });
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/refresh"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({"error":{"code":"session_rejected","message":"family revoked","retryable":false}})))
        .expect(1)
        .mount(&h.server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/deliveries"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&h.server)
        .await;
    let (r, _) = h.call(&home, &ask_op(), vec![]).await.unwrap();
    let _ = ui.expect("peek.show").await;
    let ask_id = r.ask_id.unwrap();
    ui.request(
        &AnswerOp {
            send_id: r.send_id,
            ask_id: ask_id.clone(),
            value: json!("2"),
            via: UiAnswerVia::Click,
        },
        vec![],
    )
    .await
    .unwrap();
    let db = h.db();
    eventually(10, "authority_required", || {
        query_one::<String>(
            &db,
            "SELECT status FROM outbox WHERE subject_id = ?1",
            &[&ask_id.as_str()],
        )
        .as_deref()
            == Some("authority_required")
    })
    .await;
    assert!(
        home.store
            .read_session()
            .unwrap()
            .slots
            .values()
            .all(|s| s.rejected.is_some())
    );
}

#[tokio::test]
async fn the_binary_serves_until_sigterm_and_cleans_up() {
    let dir = tempfile::tempdir().unwrap();
    let run = dir.path().join("run");
    std::fs::create_dir_all(&run).unwrap();
    let socket = run.join("peekd.sock");
    let support = dir.path().join("support");
    let exe = env!("CARGO_BIN_EXE_peekd");

    let out = std::process::Command::new(exe)
        .arg("--version")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        format!("peekd {}", silicon_peek_client::VERSION)
    );
    let out = std::process::Command::new(exe)
        .arg("bogus")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("Usage:"));

    let mut child = tokio::process::Command::new(exe)
        .args(["run", "--headless"])
        .env("PEEK_DAEMON_SOCKET", &socket)
        .env("PEEK_SUPPORT_DIR", &support)
        .env("PEEK_APPLICATIONS_DIR", dir.path().join("Applications"))
        .env("PEEK_API_URL", "http://127.0.0.1:9")
        .env("PEEK_TELEMETRY", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut conn = None;
    for _ in 0..100 {
        if let Ok(mut c) = DaemonConnection::connect(&socket, Duration::from_millis(200)).await
            && c.hello(&Hello::cli("macos-aarch64", None), Duration::from_secs(2))
                .await
                .is_ok()
        {
            conn = Some(c);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let mut c = conn.expect("peekd came up");
    let (st, _) = c
        .call(&DaemonStatus {}, None, vec![], Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(st.socket, socket.to_string_lossy());
    assert_ne!(st.pid, std::process::id());

    // A second instance loses the lock and exits with `daemon_running` (4).
    let second = tokio::process::Command::new(exe)
        .args(["run", "--headless"])
        .env("PEEK_DAEMON_SOCKET", &socket)
        .env("PEEK_SUPPORT_DIR", &support)
        .env("PEEK_TELEMETRY", "0")
        .output()
        .await
        .unwrap();
    assert_eq!(second.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&second.stderr).contains("another peekd"));

    let pid = i32::try_from(child.id().unwrap()).unwrap();
    std::process::Command::new("/bin/kill")
        .args(["-TERM", &pid.to_string()])
        .status()
        .unwrap();
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status.code(), Some(0), "SIGTERM is a clean exit");
    assert!(!socket.exists(), "the socket is removed");
    let log = std::fs::read_to_string(support.join("peekd.log")).unwrap();
    assert!(
        log.contains("peekd started")
            && log.contains("SIGTERM received")
            && log.contains("peekd stopped")
    );
    assert!(
        !log.contains("oat_") && !log.contains("jwt"),
        "no tokens in the log"
    );
}
