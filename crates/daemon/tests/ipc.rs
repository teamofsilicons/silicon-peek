//! The server side of IPC v1: handshake and negotiation, role checks, the
//! CLI `auth` block checks in order, and slot registration.

// Short names (h, c, e, r) keep the request/assert pairs readable.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::many_single_char_names
)]

mod common;

use std::time::Duration;

use common::{FakeUi, Harness};
use serde::{Deserialize, Serialize};
use serde_json::json;
use silicon_peek_client::{
    ErrorCode, Secret,
    identity::{ApiUrl, SlotIndex},
    ipc::{
        Empty, Message, Op, Request,
        cli::{CliHello, DaemonStatus, Hello, RegisterSide, StatusOp},
        frame::{AsyncFrameReader, FrameLimits, write_frame_async},
    },
    runtime::{daemon::DaemonConnection, session::Rejection},
};
use silicon_peek_daemon::config::UiExecutableRule;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Bogus {}
impl Op for Bogus {
    const NAME: &'static str = "bogus.op";
    type Output = Empty;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AnswerOnCli {
    slot: u8,
}
impl Op for AnswerOnCli {
    const NAME: &'static str = "focus";
    type Output = Empty;
}

async fn raw_conn(h: &Harness) -> DaemonConnection {
    DaemonConnection::connect(h.handle().socket_path(), Duration::from_secs(2))
        .await
        .unwrap()
}

#[tokio::test]
async fn hello_negotiates_and_reports_the_app() {
    let h = Harness::start().await;
    let mut c = raw_conn(&h).await;
    let r = c
        .hello(&Hello::cli("macos-aarch64", None), Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(r.protocol, 1);
    assert_eq!(r.peekd_version, silicon_peek_client::VERSION);
    let app = r.app.unwrap();
    assert!(!app.ui_running);
    assert_eq!(
        app.build,
        silicon_peek_client::ipc::cli::AppOfferInfo::build_number(silicon_peek_client::VERSION)
            .unwrap(),
        "no installed app: peekd reports its own build"
    );

    // A CLI that only speaks a newer protocol: peekd is older → app_update_pending.
    let mut c = raw_conn(&h).await;
    let newer = Hello::Cli(CliHello {
        cli_version: "9.0.0".into(),
        protocols: vec![2],
        platform: "macos-aarch64".into(),
        bundled_app: None,
    });
    let e = c
        .call(&newer, None, vec![], Duration::from_secs(2))
        .await
        .err()
        .unwrap();
    assert_eq!(*e.code(), ErrorCode::AppUpdatePending);
    assert!(e.retryable());

    // A CLI that only speaks an older protocol → cli_outdated naming Honeycomb.
    let mut c = raw_conn(&h).await;
    let older = Hello::Cli(CliHello {
        cli_version: "0.0.1".into(),
        protocols: vec![0],
        platform: "macos-aarch64".into(),
        bundled_app: None,
    });
    let e = c
        .call(&older, None, vec![], Duration::from_secs(2))
        .await
        .err()
        .unwrap();
    assert_eq!(*e.code(), ErrorCode::CliOutdated);
    assert!(e.hint().unwrap().contains("honeycomb update 'peek'"));
    assert_eq!(e.exit_code().code(), 4);
}

#[tokio::test]
async fn the_first_frame_must_be_hello_and_ops_are_checked_by_role() {
    let h = Harness::start().await;
    // Raw socket: a `status` before `hello`.
    let stream = tokio::net::UnixStream::connect(h.handle().socket_path())
        .await
        .unwrap();
    let (r, mut w) = stream.into_split();
    let req = Request::new(&StatusOp {}, None, vec![]).unwrap();
    write_frame_async(
        &mut w,
        &Message::Request(req).into_frame().unwrap(),
        &FrameLimits::V1,
    )
    .await
    .unwrap();
    let mut reader = AsyncFrameReader::new(r);
    let frame = reader.read_frame().await.unwrap().unwrap();
    let Message::Reply(reply) = Message::from_frame(frame).unwrap() else {
        panic!("expected a reply")
    };
    let e = reply.into_result::<serde_json::Value>().err().unwrap();
    assert_eq!(*e.code(), ErrorCode::ProtocolError);

    // Unknown ops are refused by name; UI ops are not available to CLIs.
    let mut c = h.cli().await;
    let e = c
        .call(&Bogus {}, None, vec![], Duration::from_secs(2))
        .await
        .err()
        .unwrap();
    assert_eq!(*e.code(), ErrorCode::UnknownOp);
    assert!(e.message().contains("bogus.op"));
    let e = c
        .call(
            &AnswerOnCli { slot: 1 },
            None,
            vec![],
            Duration::from_secs(2),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(*e.code(), ErrorCode::UnknownOp);
    assert!(e.message().contains("only available to Peek.app"));

    // daemon.status needs no auth.
    let (st, _) = c
        .call(&DaemonStatus {}, None, vec![], Duration::from_secs(2))
        .await
        .unwrap();
    assert!(st.running);
    assert_eq!(st.pid, std::process::id());
    assert_eq!(st.protocol, 1);
    assert!(!st.ui.running);
    assert_eq!(st.homes, 0);

    // A second hello on the same connection is a protocol error.
    let e = c
        .call(
            &Hello::cli("macos-aarch64", None),
            None,
            vec![],
            Duration::from_secs(2),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(*e.code(), ErrorCode::ProtocolError);
}

#[tokio::test]
async fn only_peek_app_may_connect_as_the_ui() {
    let h = Harness::start_with(|c| {
        c.ui_executable =
            UiExecutableRule::Exact("/Applications/Peek.app/Contents/MacOS/Peek".into());
    })
    .await;
    let e = FakeUi::try_connect(h.handle().socket_path())
        .await
        .err()
        .unwrap();
    assert_eq!(*e.code(), ErrorCode::DaemonIdentityMismatch);
    assert!(e.message().contains("only Peek.app"));
    assert!(!h.handle().ui_connected());

    // The same test binary is accepted when it is the expected executable.
    let ok = Harness::start().await;
    let ui = ok.ui().await;
    common::eventually(5, "the UI link", || ok.handle().ui_connected()).await;
    let state = ui.expect("slots.state").await;
    assert_eq!(state.fields["slots"], json!([]));
}

#[tokio::test]
async fn auth_block_checks_run_in_order() {
    let h = Harness::start().await;
    let home = h.home("si:cleanup");

    // Missing auth.
    let mut c = h.cli().await;
    let e = c
        .call(&StatusOp {}, None, vec![], Duration::from_secs(2))
        .await
        .err()
        .unwrap();
    assert_eq!(*e.code(), ErrorCode::InvalidInput);

    // Check 2: the home token.
    let bad = home.with_token(&"f".repeat(64));
    let e = c
        .call(&StatusOp {}, Some(&bad), vec![], Duration::from_secs(2))
        .await
        .err()
        .unwrap();
    assert_eq!(*e.code(), ErrorCode::HomeTokenMismatch);
    assert_eq!(e.exit_code().code(), 3);

    // Check 3: a slot for this api#context.
    let mut other_api = home.auth.clone();
    other_api.api_url = ApiUrl::parse("http://127.0.0.1:9").unwrap();
    let e = c
        .call(
            &StatusOp {},
            Some(&other_api),
            vec![],
            Duration::from_secs(2),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(*e.code(), ErrorCode::NotLoggedIn);

    // Check 1 wins over check 2: a group-readable home fails first.
    home.chmod(0o755);
    let e = c
        .call(&StatusOp {}, Some(&bad), vec![], Duration::from_secs(2))
        .await
        .err()
        .unwrap();
    assert_eq!(*e.code(), ErrorCode::InvalidSiliconHome);
    home.chmod(0o700);

    // A rejected slot.
    home.edit_session(|s| {
        for slot in s.slots.values_mut() {
            slot.rejected = Some(Rejection {
                code: "session_rejected".into(),
                at: 1,
                request_id: Some("req_x".into()),
            });
        }
    });
    let e = c
        .call(
            &StatusOp {},
            Some(&home.auth),
            vec![],
            Duration::from_secs(2),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(*e.code(), ErrorCode::SessionRejected);

    // A good block authenticates, and naming another profile does not.
    let fresh = h.home("si:fresh");
    let (st, _) = h.call(&fresh, &StatusOp {}, vec![]).await.unwrap();
    assert!(st.slot.is_none());
    assert_eq!(st.pending_asks, 0);
    let mut stolen = fresh.auth.clone();
    stolen.home_token = Secret::new(home.auth.home_token.expose());
    let mut c = h.cli().await;
    let e = c
        .call(&StatusOp {}, Some(&stolen), vec![], Duration::from_secs(5))
        .await
        .err()
        .unwrap();
    assert_eq!(*e.code(), ErrorCode::HomeTokenMismatch);
}

#[tokio::test]
async fn register_side_conflicts_and_moves() {
    let h = Harness::start().await;
    let ui = h.ui().await;
    let _ = ui.expect("slots.state").await;
    let alpha = h.home("si:alpha");
    let beta = h.home("si:beta");

    let (r, _) = h
        .call(
            &alpha,
            &RegisterSide {
                index: SlotIndex::new(3).unwrap(),
            },
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(r.slot.index.get(), 3);
    assert_eq!(serde_json::to_value(r.slot.side).unwrap(), json!("right"));
    assert_eq!(r.moved_from, None);
    assert_eq!(r.hotkey.as_deref(), Some("ctrl+cmd+3"));
    let state = ui.expect("slots.state").await;
    assert_eq!(state.fields["slots"][0]["index"], 3);
    assert_eq!(state.fields["slots"][0]["actor_id"], "si:alpha");
    assert_eq!(
        state.fields["slots"][0]["initial"], "C",
        "from the display name"
    );
    assert_eq!(state.fields["slots"][0]["hotkey"], true);
    assert_eq!(state.fields["slots"][0]["drawing"], json!(null));

    // Beta cannot take 3.
    let e = h
        .call(
            &beta,
            &RegisterSide {
                index: SlotIndex::new(3).unwrap(),
            },
            vec![],
        )
        .await
        .err()
        .unwrap();
    assert_eq!(*e.code(), ErrorCode::SideTaken);
    assert_eq!(e.exit_code().code(), 4);
    assert_eq!(
        e.message(),
        "position 3 is held by si:alpha; peek gives each Silicon exactly one position."
    );
    let d = e.details().unwrap();
    assert_eq!(d["owner"], "si:alpha");
    assert_eq!(d["free"], json!([1, 2, 4, 5, 6, 7, 8]));
    assert!(e.hint().unwrap().contains("peek register side 1"));

    // Alpha moves to 5; 3 frees up.
    let (r, _) = h
        .call(
            &alpha,
            &RegisterSide {
                index: SlotIndex::new(5).unwrap(),
            },
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(r.moved_from.map(SlotIndex::get), Some(3));
    let (r, _) = h
        .call(
            &beta,
            &RegisterSide {
                index: SlotIndex::new(3).unwrap(),
            },
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(r.slot.index.get(), 3);
    // Re-registering the same slot is a no-op.
    let (r, _) = h
        .call(
            &alpha,
            &RegisterSide {
                index: SlotIndex::new(5).unwrap(),
            },
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(r.moved_from, None);

    let (st, _) = h.call(&alpha, &StatusOp {}, vec![]).await.unwrap();
    assert_eq!(st.slot.map(|s| s.index.get()), Some(5));
    assert!(st.ui_running);
    let db = h.db();
    let n: i64 = common::query_one(&db, "SELECT count(*) FROM slots", &[]).unwrap();
    assert_eq!(n, 2);
    let homes: i64 = common::query_one(&db, "SELECT count(*) FROM homes", &[]).unwrap();
    assert_eq!(homes, 2, "every authenticated home is recorded");
    let json = std::fs::read_to_string(h.cfg.support_dir.join("homes.json")).unwrap();
    assert!(json.contains("si:alpha") && json.contains("si:beta"));
}
