//! 0.1.2 commands over a fake peekd (contract §5, test list §12.2): the
//! feature gate against an older peekd, `queue_full`, `peek queue`,
//! `peek cancel`, `peek schedule …`, and the new `peek send` flags on the
//! wire.
#![cfg(target_os = "macos")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::Arc;

use common::{
    DEAD_API, Env,
    daemon::{self, FakeDaemon, Handler},
};
use serde_json::{Value, json};
use silicon_peek_client::{
    ids::{ScheduleId, SendId},
    timestamp::{Timestamp, unix_now},
};

fn logged_in() -> Env {
    let env = Env::new(DEAD_API);
    env.login_as("oat_q", "ort_q", unix_now() + 1800);
    env
}

fn send_reply(req: &silicon_peek_client::ipc::Request) -> Value {
    let due = req.fields.get("due_at").cloned().unwrap_or(Value::Null);
    let scheduled = !due.is_null();
    json!({"send_id": SendId::generate().to_string(), "ask_id": null, "slot": 3,
        "status": if scheduled { "scheduled" } else { "queued" },
        "speech": null, "warnings": [],
        "queue_position": if scheduled { Value::Null } else { json!(2) },
        "waiting": if scheduled { Value::Null } else { json!(2) },
        "expires_at": req.fields.get("expires_at").cloned().unwrap_or(Value::Null),
        "schedule_id": if scheduled { json!(ScheduleId::generate().to_string()) } else { Value::Null },
        "due_at": due,
        "tz": req.fields.get("tz").cloned().unwrap_or(Value::Null),
        "replaced_send_id": null})
}

fn handler() -> Handler {
    Arc::new(|req| match req.op.as_str() {
        "send" => vec![daemon::ok(req, send_reply(req))],
        "config.sync" => vec![daemon::ok(req, json!({}))],
        "queue.list" => vec![daemon::ok(
            req,
            json!({"slot": 3, "limit": 5, "held": null, "scheduled": 1, "scheduled_limit": 500,
                "on_screen": {"send_id": SendId::generate().to_string(), "ask_id": null, "kind": "show",
                    "summary": "Build finished", "state": "on_screen", "queue_position": 0,
                    "created_at": "2026-09-27T06:11:48.000Z", "queued_at": "2026-09-27T06:11:48.000Z",
                    "age_ms": 12000, "expires_at": null, "shown_at": "2026-09-27T06:11:49.000Z",
                    "schedule_id": null, "due_at": null},
                "waiting": []}),
        )],
        "queue.clear" => vec![daemon::ok(
            req,
            json!({"cancelled": [], "on_screen": null, "on_screen_cancelled": false}),
        )],
        "send.cancel" => vec![daemon::ok(
            req,
            json!({"send_id": req.fields["target"], "ask_id": null, "schedule_id": null,
                "was": "waiting", "queue_position": 2, "state": "cancelled", "due_at": null, "tz": null}),
        )],
        "schedule.list" => vec![daemon::ok(
            req,
            json!({"scheduled": [], "limit": 500, "slot": 3}),
        )],
        "schedule.cancel" => vec![daemon::err(
            req,
            json!({"code": "schedule_not_found", "message": "no scheduled send", "retryable": false,
                "hint": "peek schedule list    lists this Silicon's scheduled sends"}),
        )],
        "schedule.clear" => vec![daemon::ok(req, json!({"cancelled": []}))],
        _ => vec![daemon::err(
            req,
            json!({"code":"unknown_op","message":"no"}),
        )],
    })
}

fn legacy(env: &Env) -> FakeDaemon {
    daemon::start(&env.socket, handler())
}

fn v2(env: &Env) -> FakeDaemon {
    daemon::start_v2(&env.socket, handler())
}

const SHOW: &str = r#"{"elements":[{"type":"text","text":"hi"}]}"#;

/// Tomorrow's UTC date (`YYYY-MM-DD`), so date-times stay in the future.
fn tomorrow() -> String {
    Timestamp::now()
        .plus(std::time::Duration::from_hours(24))
        .to_rfc3339()[..10]
        .to_owned()
}

/// An RFC 3339 instant `secs` from now.
fn in_secs(secs: u64) -> String {
    Timestamp::now()
        .plus(std::time::Duration::from_secs(secs))
        .to_rfc3339()
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // one table of cases
async fn new_flags_are_refused_by_an_older_peekd() {
    let env = logged_in();
    let d = legacy(&env);
    let in_an_hour = in_secs(3600);
    let cases: [(&[&str], &str, &str); 6] = [
        (
            &["send", "--show", SHOW, "--expires-in", "10m"],
            "expiry_all",
            "--expires-in/--expires-at on --speak and --show",
        ),
        (
            &[
                "send",
                "--ask",
                r#"{"question":"q","type":"text"}"#,
                "--expires-at",
                &in_an_hour,
            ],
            "expiry_all",
            "--expires-in/--expires-at on --speak and --show",
        ),
        (
            &["send", "--speak", "x", "--in", "5m"],
            "schedule",
            "scheduled sends (--in/--at)",
        ),
        (
            &["send", "--speak", "x", "--replace"],
            "replace",
            "--replace",
        ),
        (
            &["send", "--speak", "x", "--notify", "shown"],
            "notify_shown",
            "--notify shown",
        ),
        (&["queue"], "queue_v2", "peek queue / peek cancel"),
    ];
    for (args, feature, what) in cases {
        let mut full = args.to_vec();
        full.push("--json");
        let run = env.run(&full).await;
        assert_eq!(run.code, 5, "{args:?}: {}", run.stderr);
        assert!(run.stdout.is_empty());
        let e = run.error();
        assert_eq!(e["code"], "app_update_pending", "{args:?}");
        assert_eq!(e["retryable"], true);
        assert_eq!(
            e["details"]["missing_features"],
            json!([feature]),
            "{args:?}"
        );
        assert_eq!(e["details"]["peekd_version"], "0.1.0");
        assert_eq!(
            e["message"],
            format!(
                "Peek.app on this Mac runs peekd 0.1.0, which does not support {what} yet; Peek.app updates itself when nothing is on screen"
            )
        );
        assert_eq!(
            e["hint"],
            "run `peek app update` to apply the bundled build now, then retry"
        );
    }
    for args in [
        &["cancel", "snd_0192a000000070008000000000000001"][..],
        &["queue", "clear"],
        &["schedule", "list"],
        &["schedule", "clear"],
    ] {
        let mut full = args.to_vec();
        full.push("--json");
        let run = env.run(&full).await;
        assert_eq!(run.code, 5, "{args:?}");
        assert_eq!(run.error()["code"], "app_update_pending");
    }
    // Nothing reached the old peekd.
    assert!(
        d.seen()
            .iter()
            .all(|s| s.op != "send" && !s.op.starts_with("queue"))
    );
    // A plain send and --expires-in on an ask (0.1.1 had it) still go out.
    let run = env.run(&["send", "--speak", "plain", "--json"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    let run = env
        .run(&[
            "send",
            "--ask",
            r#"{"question":"q","type":"text"}"#,
            "--expires-in",
            "60",
            "--json",
        ])
        .await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    let sends: Vec<_> = d.seen().into_iter().filter(|s| s.op == "send").collect();
    assert_eq!(sends.len(), 2);
    for key in ["expires_at", "due_at", "tz", "replace"] {
        assert!(
            !sends[0].fields.contains_key(key),
            "{key} stays off a plain send"
        );
    }
    assert_eq!(sends[1].fields["expires_in_s"], 60);
}

#[tokio::test]
async fn a_config_default_shown_is_dropped_for_an_older_peekd() {
    let env = logged_in();
    let set = env
        .run(&[
            "config",
            "set",
            r#"{"notify":["shown","speech_finished"]}"#,
            "--json",
        ])
        .await;
    assert_eq!(set.code, 0, "{}", set.stderr);
    let d = legacy(&env);
    let run = env.run(&["send", "--speak", "hi"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(
        run.stderr
            .contains("config notify \"shown\" is not supported by peekd 0.1.0"),
        "{}",
        run.stderr
    );
    let sends: Vec<_> = d.seen().into_iter().filter(|s| s.op == "send").collect();
    assert_eq!(sends[0].fields["notify"], json!(["speech_finished"]));
    drop(d);
    std::fs::remove_file(&env.socket).ok();
    let d = v2(&env);
    let run = env.run(&["send", "--speak", "hi", "--json"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    let sends: Vec<_> = d.seen().into_iter().filter(|s| s.op == "send").collect();
    assert_eq!(
        sends[0].fields["notify"],
        json!(["speech_finished", "shown"])
    );
}

#[tokio::test]
async fn new_send_flags_reach_peekd_and_json_keeps_every_key() {
    let env = logged_in();
    let d = v2(&env);
    let day = tomorrow();
    let at = format!("{day}T09:00");
    let expires = format!("{day}T10:00");
    let run = env
        .run(&[
            "send",
            "--speak",
            "later",
            "--at",
            &at,
            "--tz",
            "Asia/Kolkata",
            "--expires-at",
            &expires,
            "--replace",
            "--json",
        ])
        .await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    let v = run.json();
    for key in [
        "send_id",
        "ask_id",
        "slot",
        "status",
        "speech",
        "warnings",
        "queue_position",
        "waiting",
        "expires_at",
        "schedule_id",
        "due_at",
        "tz",
        "replaced_send_id",
    ] {
        assert!(v.get(key).is_some(), "{key} is always present");
    }
    assert_eq!(v["status"], "scheduled");
    let send = d.seen().into_iter().find(|s| s.op == "send").unwrap();
    assert_eq!(
        send.fields["due_at"],
        format!("{day}T03:30:00.000Z"),
        "09:00 IST"
    );
    assert_eq!(send.fields["expires_at"], format!("{day}T04:30:00.000Z"));
    assert_eq!(send.fields["tz"], "Asia/Kolkata");
    assert_eq!(send.fields["replace"], true);
    assert!(send.fields["expires_in_s"].is_null());
    // --in becomes an absolute due time.
    let before = Timestamp::now();
    let run = env
        .run(&["send", "--speak", "soon", "--in", "90s", "--json"])
        .await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    let send = d
        .seen()
        .into_iter()
        .filter(|s| s.op == "send")
        .nth(1)
        .unwrap();
    let due = Timestamp::parse(send.fields["due_at"].as_str().unwrap()).unwrap();
    assert!(
        due.unix_ms() >= before.unix_ms() + 90_000 && due.unix_ms() < before.unix_ms() + 100_000
    );
    // Human output of a waiting send points at `peek queue`.
    let run = env
        .run(&["send", "--show", SHOW, "--expires-in", "2h"])
        .await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(
        run.stdout.contains("(queued: 2 ahead of it)"),
        "{}",
        run.stdout
    );
    assert!(run.stderr.contains("Next: peek queue"), "{}", run.stderr);
    let send = d
        .seen()
        .into_iter()
        .filter(|s| s.op == "send")
        .nth(2)
        .unwrap();
    assert_eq!(send.fields["expires_in_s"], 7200);
}

#[tokio::test]
async fn input_errors_come_before_peekd() {
    let env = logged_in();
    let d = v2(&env);
    for (args, code, field) in [
        (
            &[
                "send",
                "--speak",
                "x",
                "--at",
                "09:00",
                "--expires-in",
                "10m",
            ][..],
            "conflicting_flags",
            None,
        ),
        (
            &["send", "--speak", "x", "--tz", "Asia/Kolkata"],
            "conflicting_flags",
            None,
        ),
        (
            &["send", "--speak", "x", "--at", "2001-01-01T09:00Z"],
            "invalid_input",
            Some("--at"),
        ),
        (
            &["send", "--speak", "x", "--at", "6pm"],
            "invalid_input",
            Some("--at"),
        ),
        (
            &["send", "--speak", "x", "--in", "1w"],
            "invalid_input",
            Some("--in"),
        ),
        (
            &[
                "send",
                "--speak",
                "x",
                "--at",
                "10:00",
                "--tz",
                "Asia/Kolkatta",
            ],
            "invalid_input",
            Some("--tz"),
        ),
        (
            &["send", "--speak", "x", "--expires-in", "5s"],
            "invalid_input",
            Some("--expires-in"),
        ),
    ] {
        let mut full = args.to_vec();
        full.push("--json");
        let run = env.run(&full).await;
        assert_eq!(run.code, 2, "{args:?}: {}", run.stderr);
        let e = run.error();
        assert_eq!(e["code"], code, "{args:?}");
        assert!(e["hint"].is_string(), "{args:?}");
        if let Some(f) = field {
            assert_eq!(e["details"]["field"], f, "{args:?}");
        }
    }
    assert!(d.seen().iter().all(|s| s.op != "send"));
}

#[tokio::test]
async fn queue_full_is_exit_4_with_the_json_error() {
    let env = logged_in();
    let _d = daemon::start_v2(
        &env.socket,
        Arc::new(|req| {
            vec![daemon::err(
                req,
                json!({"code": "queue_full", "retryable": true,
                    "message": "position 3's queue is full: 1 send on screen and 5 waiting (at most 5); remove one with `peek cancel <send_id>` or `peek queue clear`",
                    "hint": "peek queue    lists the waiting sends and their IDs",
                    "details": {"queued": 5, "limit": 5, "on_screen": "snd_x", "waiting": [], "due_waiting": 0, "held": null}}),
            )]
        }),
    );
    let run = env
        .run(&["send", "--speak", "one too many", "--json"])
        .await;
    assert_eq!(run.code, 4, "{}", run.stderr);
    assert!(run.stdout.is_empty());
    let e = run.error();
    assert_eq!(e["code"], "queue_full");
    assert_eq!(e["details"]["limit"], 5);
    let human = env.run(&["send", "--speak", "one too many"]).await;
    assert_eq!(human.code, 4);
    assert!(
        human.stderr.contains("error: position 3's queue is full"),
        "{}",
        human.stderr
    );
    assert!(
        human.stderr.contains("hint: peek queue"),
        "{}",
        human.stderr
    );
}

#[tokio::test]
async fn queue_cancel_and_schedule_use_their_ops() {
    let env = logged_in();
    let d = v2(&env);
    let run = env.run(&["queue", "--json"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    let expected = handler()(
        &silicon_peek_client::ipc::Request::new(
            &silicon_peek_client::ipc::cli::QueueList {},
            None,
            vec![],
        )
        .unwrap(),
    );
    let silicon_peek_client::ipc::Message::Reply(reply) = &expected[0] else {
        panic!("a reply");
    };
    let mut want = reply.outcome.clone().unwrap();
    // The fake mints a fresh ID per call; compare the shape without it.
    let mut got = run.json();
    got["on_screen"]["send_id"] = json!("x");
    want["on_screen"]["send_id"] = json!("x");
    assert_eq!(got, want, "stdout is exactly the op result");
    let human = env.run(&["queue"]).await;
    assert!(
        human.stdout.starts_with(
            "position 3 · 1 on screen · 0 waiting (limit 5) · 1 scheduled\n  on screen  snd_"
        ),
        "{}",
        human.stdout
    );
    let run = env.run(&["queue", "list", "--json"]).await;
    assert_eq!(run.code, 0);
    let run = env.run(&["queue", "clear", "--all"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.stdout.trim(), "nothing was waiting");
    let id = SendId::generate().to_string();
    let run = env.run(&["cancel", &id]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(
        run.stdout.trim(),
        format!("{id} cancelled; it was #2 in line and will not be shown")
    );
    let run = env.run(&["schedule", "list"]).await;
    assert_eq!(run.stdout.trim(), "nothing scheduled");
    let run = env.run(&["schedule", "clear"]).await;
    assert_eq!(run.stdout.trim(), "nothing was scheduled");
    let run = env
        .run(&[
            "schedule",
            "cancel",
            &ScheduleId::generate().to_string(),
            "--json",
        ])
        .await;
    assert_eq!(run.code, 4);
    assert_eq!(run.error()["code"], "schedule_not_found");
    let ops: Vec<String> = d.seen().into_iter().map(|s| s.op).collect();
    for op in [
        "queue.list",
        "queue.clear",
        "send.cancel",
        "schedule.list",
        "schedule.clear",
        "schedule.cancel",
    ] {
        assert!(ops.iter().any(|o| o == op), "{op}");
    }
    let clear = d
        .seen()
        .into_iter()
        .find(|s| s.op == "queue.clear")
        .unwrap();
    assert_eq!(clear.fields["all"], true);
    let cancel = d
        .seen()
        .into_iter()
        .find(|s| s.op == "send.cancel")
        .unwrap();
    assert_eq!(cancel.fields["target"], id);
    assert!(
        cancel.auth.is_some(),
        "every queue op carries the home's auth"
    );
}
