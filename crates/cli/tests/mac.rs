//! Mac-bound commands over a fake peekd (BLUEPRINT §1.6, §7.4): every
//! `peek send` limit is refused locally with the right code and exit before
//! peekd is contacted; the happy path sends image bytes as blobs with the
//! home's auth block; `--wait`; `register side` (including the `side_taken`
//! error shape in both output modes); status, unregister, ask and daemon ops.
#![cfg(target_os = "macos")]

mod common;

use std::sync::Arc;

use common::{
    DEAD_API, Env,
    daemon::{self, FakeDaemon, Handler},
    png_bytes, write_file,
};
use serde_json::{Value, json};
use silicon_peek_client::{
    ids::{AskId, SendId},
    timestamp::unix_now,
};

fn logged_in() -> Env {
    let env = Env::new(DEAD_API);
    env.login_as("oat_m", "ort_m", unix_now() + 1800);
    env
}

fn send_ok(ask_id: Option<&str>) -> Handler {
    let ask = ask_id.map(str::to_owned);
    Arc::new(move |req| match req.op.as_str() {
        "send" => vec![daemon::ok(
            req,
            json!({"send_id": SendId::generate().to_string(),
                   "ask_id": ask, "slot": 3, "status": "showing",
                   "speech": {"status": "pending", "model": "aura-2-thalia-en", "chars": 5},
                   "warnings": []}),
        )],
        _ => vec![daemon::err(
            req,
            json!({"code":"unknown_op","message":"no"}),
        )],
    })
}

fn fake(env: &Env, handler: Handler) -> FakeDaemon {
    daemon::start(&env.socket, handler)
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // one table row per limit
async fn every_send_limit_is_refused_locally() {
    let env = logged_in();
    let d = fake(&env, send_ok(None));
    let dir = env.home.clone();
    let png = write_file(&dir, "ok.png", &png_bytes());
    let note = write_file(&dir, "note.txt", b"not an image");
    let big = dir.join("big.png");
    {
        let mut bytes = png_bytes();
        bytes.resize(10 * 1024 * 1024 + 1, 0);
        std::fs::write(&big, bytes).unwrap_or_else(|e| panic!("{e}"));
    }
    let text = |n: usize| {
        format!(
            r#"{{"elements":[{{"type":"text","text":"{}"}}]}}"#,
            "t".repeat(n)
        )
    };
    let long_caption = format!(
        r#"{{"elements":[{{"type":"image","path":"{}","caption":"{}"}}]}}"#,
        png.display(),
        "c".repeat(51)
    );
    let question = |n: usize| format!(r#"{{"question":"{}","type":"text"}}"#, "q".repeat(n));
    let seven =
        r#"{"question":"q","type":"single_choice","options":["1","2","3","4","5","6","7"]}"#;
    let four = r#"{"elements":[{"type":"text","text":"a"},{"type":"text","text":"b"},{"type":"text","text":"c"},{"type":"text","text":"d"}]}"#;
    let img = |p: &std::path::Path| {
        format!(
            r#"{{"elements":[{{"type":"image","path":"{}"}}]}}"#,
            p.display()
        )
    };
    let ask = r#"{"question":"Ship?","type":"single_choice","options":["Ship","Hold"]}"#;
    let cases: Vec<(Vec<String>, &str, i32)> = vec![
        (vec![], "nothing_to_send", 2),
        (
            vec!["--show".into(), text(1), "--ask".into(), ask.into()],
            "conflicting_flags",
            2,
        ),
        (
            vec!["--speak".into(), "s".repeat(2001)],
            "speak_too_long",
            2,
        ),
        (vec!["--show".into(), text(161)], "text_too_long", 2),
        (vec!["--show".into(), long_caption], "caption_too_long", 2),
        (vec!["--ask".into(), question(81)], "question_too_long", 2),
        (vec!["--show".into(), four.into()], "too_many_elements", 2),
        (vec!["--ask".into(), seven.into()], "too_many_options", 2),
        (
            vec!["--show".into(), img(&dir.join("missing.png"))],
            "image_unreadable",
            2,
        ),
        (vec!["--show".into(), img(&big)], "image_too_large", 2),
        (vec!["--show".into(), img(&note)], "image_unsupported", 2),
        (vec!["--show".into(), "{not json".into()], "invalid_json", 2),
        (
            vec!["--show".into(), r#"{"elements":[],"elements":[]}"#.into()],
            "invalid_json",
            2,
        ),
        (
            vec![
                "--show".into(),
                r#"{"elements":[{"type":"text","text":"a"}],"extra":1}"#.into(),
            ],
            "invalid_input",
            2,
        ),
        (
            vec![
                "--speak".into(),
                "hi".into(),
                "--duration".into(),
                "0".into(),
            ],
            "invalid_input",
            2,
        ),
        (
            vec![
                "--speak".into(),
                "hi".into(),
                "--duration".into(),
                "121".into(),
            ],
            "invalid_input",
            2,
        ),
        (
            vec![
                "--ask".into(),
                ask.into(),
                "--expires-in".into(),
                "9".into(),
            ],
            "invalid_input",
            2,
        ),
        (
            vec!["--ask".into(), ask.into(), "--wait=601".into()],
            "invalid_input",
            2,
        ),
        (
            vec!["--speak".into(), "hi".into(), "--wait=5".into()],
            "conflicting_flags",
            2,
        ),
        (
            vec![
                "--speak".into(),
                "hi".into(),
                "--voice".into(),
                "thalia".into(),
            ],
            "invalid_input",
            2,
        ),
        (
            vec![
                "--show".into(),
                text(1),
                "--voice".into(),
                "aura-2-thalia-en".into(),
            ],
            "conflicting_flags",
            2,
        ),
        (
            vec![
                "--speak".into(),
                "hi".into(),
                "--notify".into(),
                "everything".into(),
            ],
            "invalid_input",
            2,
        ),
    ];
    for (args, code, exit) in cases {
        let mut argv = vec!["send", "--json"];
        argv.extend(args.iter().map(String::as_str));
        let run = env.run(&argv).await;
        assert_eq!(run.code, exit, "{code}: {}", run.stderr);
        assert!(run.stdout.is_empty(), "{code}: stdout must be empty");
        assert_eq!(run.error()["code"], code, "{args:?}");
    }
    assert!(d.seen().is_empty(), "nothing reached peekd: {:?}", d.seen());
}

#[tokio::test]
async fn send_requires_login_before_contacting_peekd() {
    let env = Env::new(DEAD_API);
    let d = fake(&env, send_ok(None));
    let run = env.run(&["send", "--speak", "hi", "--json"]).await;
    assert_eq!(run.code, 3);
    assert_eq!(run.error()["code"], "not_logged_in");
    assert!(d.seen().is_empty());
}

#[tokio::test]
async fn send_ships_bytes_as_blobs_with_the_home_auth() {
    let mut env = logged_in();
    env.var("ISI", "deliberate:session-1");
    let d = fake(&env, send_ok(None));
    let png = write_file(&env.home, "cover.png", &png_bytes());
    let run = env
        .run(&[
            "send",
            "--json",
            "--speak",
            "Now playing",
            "--show",
            r#"{"elements":[{"type":"text","text":"CO2"},{"type":"image","path":"./cover.png","caption":"Prateek"}]}"#,
            "--notify",
            "speech_finished",
            "--duration",
            "8",
        ])
        .await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    let v = run.json();
    assert_eq!(v["slot"], 3);
    assert_eq!(v["status"], "showing");
    assert_eq!(v["speech"]["status"], "pending");
    let seen = d.seen();
    let ops: Vec<&str> = seen.iter().map(|s| s.op.as_str()).collect();
    // The tests run with PEEK_TELEMETRY=0: that opt-out is forwarded to
    // peekd after the command, so peekd records nothing for this home either.
    assert_eq!(ops, vec!["send", "config.sync"]);
    assert_eq!(seen[1].fields["config"]["telemetry"], false);
    assert_eq!(seen[1].fields["config"]["env_opt_out"], true);
    assert_eq!(seen[1].auth, seen[0].auth);
    let s = &seen[0];
    assert_eq!(s.op, "send");
    assert_eq!(s.blobs, vec![std::fs::read(png).unwrap_or_default()]);
    assert_eq!(s.fields["show"]["elements"][1]["path"], json!({"blob": 0}));
    assert_eq!(s.fields["show"]["elements"][1]["caption"], "Prateek");
    assert_eq!(s.fields["speak"], "Now playing");
    assert_eq!(s.fields["isi"], "deliberate:session-1");
    assert_eq!(s.fields["notify"], json!(["speech_finished"]));
    assert_eq!(s.fields["duration_ms"], 8000);
    assert_eq!(s.fields["wait"], false);
    let auth = s.auth.clone().unwrap_or(Value::Null);
    assert_eq!(auth["home"], env.store_dir().display().to_string());
    assert_eq!(auth["api_url"], DEAD_API);
    assert_eq!(auth["context"], "production");
    let token = std::fs::read_to_string(env.store_dir().join("daemon-token")).unwrap_or_default();
    assert_eq!(auth["home_token"], token.trim());
}

#[tokio::test]
async fn send_uses_the_config_notify_default() {
    let env = logged_in();
    let d = fake(&env, send_ok(None));
    let set = env
        .run(&[
            "config",
            "set",
            r#"{"notify":["show_dismissed"]}"#,
            "--json",
        ])
        .await;
    assert_eq!(set.code, 0, "{}", set.stderr);
    let run = env
        .run(&[
            "send",
            "--json",
            "--show",
            r#"{"elements":[{"type":"text","text":"hi"}]}"#,
        ])
        .await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    let sends: Vec<_> = d.seen().into_iter().filter(|s| s.op == "send").collect();
    assert_eq!(sends[0].fields["notify"], json!(["show_dismissed"]));
}

#[tokio::test]
async fn send_wait_prints_the_answer() {
    let env = logged_in();
    let ask_id = AskId::generate().to_string();
    let for_daemon = ask_id.clone();
    let handler: Handler = Arc::new(move |req| {
        vec![
            daemon::ok(
                req,
                json!({"send_id": SendId::generate().to_string(), "ask_id": for_daemon,
                       "slot": 3, "status": "showing", "speech": null, "warnings": []}),
            ),
            daemon::event(
                "ask.result",
                &json!({"ask_id": for_daemon, "state": "answered",
                       "answer": {"kind": "single_choice", "option_id": "keep", "label": "Keep"},
                       "via": "click", "transcript": null, "answered_at": "2026-09-26T10:00:07Z"}),
            ),
        ]
    });
    let d = fake(&env, handler);
    let run = env
        .run(&[
            "send",
            "--json",
            "--ask",
            r#"{"question":"Delete old.zip?","type":"single_choice","options":[{"id":"keep","label":"Keep"},{"id":"delete","label":"Delete"}]}"#,
            "--wait=30",
        ])
        .await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    let v = run.json();
    assert_eq!(v["ask_id"], ask_id);
    assert_eq!(v["state"], "answered");
    assert_eq!(
        v["answer"],
        json!({"kind":"single_choice","option_id":"keep","label":"Keep"})
    );
    assert_eq!(v["via"], "click");
    assert!(v["transcript"].is_null());
    assert_eq!(v["answered_at"], "2026-09-26T10:00:07Z");
    assert_eq!(d.seen()[0].fields["wait"], true);
    // The CLI acknowledges the result it read, so peekd sends no ting.
    let mut acked = false;
    for _ in 0..100 {
        acked = d
            .seen()
            .iter()
            .any(|s| s.op == "ask.result.ack" && s.fields["ask_id"] == ask_id.as_str());
        if acked {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(acked, "no ask.result.ack in {:?}", d.seen());
}

#[tokio::test]
async fn send_wait_times_out_to_ting() {
    let env = logged_in();
    let ask_id = AskId::generate().to_string();
    let for_daemon = ask_id.clone();
    let handler: Handler = Arc::new(move |req| {
        vec![daemon::ok(
            req,
            json!({"send_id": SendId::generate().to_string(), "ask_id": for_daemon,
                   "slot": 3, "status": "queued", "speech": null, "warnings": []}),
        )]
    });
    let _d = fake(&env, handler);
    let run = env
        .run(&[
            "send",
            "--json",
            "--ask",
            r#"{"question":"Ok?","type":"text"}"#,
            "--wait=1",
        ])
        .await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    let v = run.json();
    assert_eq!(v["ask_id"], ask_id);
    assert_eq!(v["state"], "pending");
    assert_eq!(v["delivery"], "ting");
}

#[tokio::test]
async fn register_side_reports_the_slot_and_hotkey() {
    let env = logged_in();
    let d = fake(
        &env,
        Arc::new(|req| {
            vec![daemon::ok(
                req,
                json!({"slot": {"index": req.fields["index"], "side": "bottom"}, "moved_from": null}),
            )]
        }),
    );
    let run = env.run(&["register", "side", "5", "--json"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(
        run.json(),
        json!({"slot": {"index": 5, "side": "bottom"}, "moved_from": null, "hotkey": "ctrl+cmd+5", "warnings": []})
    );
    assert_eq!(d.seen()[0].op, "register.side");
    let out_of_range = env.run(&["register", "side", "9", "--json"]).await;
    assert_eq!(out_of_range.code, 2);
    assert_eq!(out_of_range.error()["code"], "invalid_input");
}

#[tokio::test]
async fn side_taken_keeps_peekds_error_shape() {
    let env = logged_in();
    let _d = fake(
        &env,
        Arc::new(|req| {
            vec![daemon::err(
                req,
                json!({"code": "side_taken",
                       "message": "position 3 is held by si:dj; peek gives each Silicon exactly one position.",
                       "hint": "choose a free one: peek register side 1 (free: 1,4,6,7)",
                       "retryable": false, "details": {"owner": "si:dj", "free": [1, 4, 6, 7]}}),
            )]
        }),
    );
    let run = env.run(&["register", "side", "3", "--json"]).await;
    assert_eq!(run.code, 4);
    assert!(run.stdout.is_empty());
    assert_eq!(
        run.error(),
        json!({"code": "side_taken",
               "message": "position 3 is held by si:dj; peek gives each Silicon exactly one position.",
               "hint": "choose a free one: peek register side 1 (free: 1,4,6,7)",
               "retryable": false, "request_id": null, "details": {"owner": "si:dj", "free": [1, 4, 6, 7]}})
    );
    let human = env.run(&["register", "side", "3"]).await;
    assert_eq!(human.code, 4);
    assert_eq!(
        human.stderr.lines().collect::<Vec<_>>(),
        vec![
            "error: position 3 is held by si:dj; peek gives each Silicon exactly one position.",
            "hint: choose a free one: peek register side 1 (free: 1,4,6,7)",
            "help: peek register side --help",
        ]
    );
}

#[tokio::test]
async fn register_drawing_sends_the_script_and_writes_the_preview() {
    let env = logged_in();
    let d = fake(
        &env,
        Arc::new(|req| {
            let reply = silicon_peek_client::ipc::Reply::ok(
                &req.id,
                &json!({"sha256": "ab", "bytes": req.blobs[0].len(),
                        "stats": {"frames": 90, "p50_ms": 0.31, "p95_ms": 0.58, "max_ms": 0.92, "ops_max": 212, "glass_rebuilds": 1},
                        "warnings": [], "logs": [], "active": true, "slot": 5, "server_sync": "pending"}),
                vec![b"\x89PNG preview".to_vec()],
            );
            vec![silicon_peek_client::ipc::Message::Reply(
                reply.unwrap_or_else(|e| panic!("{e}")),
            )]
        }),
    );
    let script = write_file(
        &env.home,
        "logo.js",
        b"peek.frame((ctx) => { ctx.fillRect(0,0,100,100) })",
    );
    let run = env
        .run(&["register", "drawing", "./logo.js", "--preview", "out.png"])
        .await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(run.stdout.starts_with("✓ loaded ("), "{}", run.stdout);
    assert!(
        run.stdout
            .contains("drawing active for silicon \"cleanup\" at slot 5 (bottom)")
    );
    assert_eq!(
        std::fs::read(env.home.join("out.png")).unwrap_or_default(),
        b"\x89PNG preview"
    );
    let seen = d.seen();
    assert_eq!(seen[0].op, "register.drawing");
    assert_eq!(seen[0].fields["filename"], "logo.js");
    assert_eq!(seen[0].fields["preview"], true);
    assert_eq!(seen[0].fields["check_only"], false);
    assert_eq!(
        seen[0].blobs,
        vec![std::fs::read(script).unwrap_or_default()]
    );

    let big = write_file(&env.home, "big.js", &vec![b'a'; 256 * 1024 + 1]);
    let run = env
        .run(&[
            "register",
            "drawing",
            big.to_str().unwrap_or_default(),
            "--json",
        ])
        .await;
    assert_eq!(run.code, 2);
    assert_eq!(run.error()["code"], "drawing_too_large");
}

#[tokio::test]
async fn status_unregister_ask_and_history_use_their_ops() {
    let env = logged_in();
    let d = fake(
        &env,
        Arc::new(|req| {
            let result = match req.op.as_str() {
                "status" => json!({"slot": {"index": 3, "side": "right"}, "drawing": null,
                                   "queue": {"pending": 0}, "pending_asks": 1,
                                   "deliveries": {"pending": 0, "authority_required": 0, "last_error": null},
                                   "ui_running": true}),
                "unregister" => json!({"released_slot": 3, "cancelled_asks": []}),
                "ask.list" => json!({"asks": []}),
                "history" => json!({"items": []}),
                _ => {
                    return vec![daemon::err(
                        req,
                        json!({"code":"ask_not_found","message":"no such ask"}),
                    )];
                }
            };
            vec![daemon::ok(req, result)]
        }),
    );
    let status = env.run(&["status", "--json"]).await;
    assert_eq!(status.code, 0, "{}", status.stderr);
    let v = status.json();
    assert_eq!(v["slot"]["index"], 3);
    assert_eq!(v["actor_id"], "si:cleanup");
    assert_eq!(v["daemon"]["protocol"], 1);
    let un = env.run(&["unregister", "--json"]).await;
    assert_eq!(un.json()["released_slot"], 3);
    let list = env
        .run(&[
            "ask", "list", "--state", "pending", "--limit", "20", "--json",
        ])
        .await;
    assert_eq!(list.code, 0, "{}", list.stderr);
    let history = env.run(&["history", "--limit", "5", "--json"]).await;
    assert_eq!(history.json()["items"], json!([]));
    let bad_id = env.run(&["ask", "get", "nope", "--json"]).await;
    assert_eq!(bad_id.code, 2);
    let missing = env
        .run(&["ask", "get", &AskId::generate().to_string(), "--json"])
        .await;
    assert_eq!(missing.code, 4);
    assert_eq!(missing.error()["code"], "ask_not_found");
    let too_many = env.run(&["history", "--limit", "201", "--json"]).await;
    assert_eq!(too_many.code, 2);
    // Every command that reached peekd is followed by the forwarded
    // PEEK_TELEMETRY=0 opt-out.
    let (synced, seen): (Vec<_>, Vec<_>) =
        d.seen().into_iter().partition(|s| s.op == "config.sync");
    let ops: Vec<String> = seen.iter().map(|s| s.op.clone()).collect();
    assert_eq!(
        ops,
        vec!["status", "unregister", "ask.list", "history", "ask.get"]
    );
    assert_eq!(synced.len(), 5);
    assert!(
        synced
            .iter()
            .all(|s| s.fields["config"]["telemetry"] == false)
    );
    let list_fields = &seen[2].fields;
    assert_eq!(list_fields["state"], "pending");
    assert_eq!(list_fields["limit"], 20);
}

#[tokio::test]
async fn without_peekd_and_without_launching_the_service_is_unavailable() {
    let env = logged_in();
    let run = env.run(&["register", "side", "2", "--json"]).await;
    assert_eq!(run.code, 5, "{}", run.stderr);
    assert_eq!(run.error()["code"], "peek_service_unavailable");
    assert!(
        !env.apps.join("Peek.app").exists(),
        "no app was installed from a build without Peek.app.zip"
    );
}

#[tokio::test]
async fn daemon_status_answers_without_login() {
    let env = Env::new(DEAD_API);
    let down = env.run(&["daemon", "status", "--json"]).await;
    assert_eq!(down.code, 0);
    assert_eq!(down.json()["running"], false);
    let _d = fake(
        &env,
        Arc::new(|req| {
            vec![daemon::ok(
                req,
                json!({"running": true, "pid": 42, "version": "0.1.0", "protocol": 1,
                       "socket": "/var/tmp/silicon-peek-501/peekd.sock",
                       "ui": {"running": true, "build": 1000}, "homes": 2}),
            )]
        }),
    );
    let up = env.run(&["daemon", "status", "--json"]).await;
    assert_eq!(up.code, 0, "{}", up.stderr);
    assert_eq!(up.json()["pid"], 42);
    assert_eq!(up.json()["homes"], 2);
}

#[tokio::test]
async fn app_status_reports_an_absent_app() {
    let env = Env::new(DEAD_API);
    let run = env.run(&["app", "status", "--json"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    let v = run.json();
    assert_eq!(v["supported"], true);
    assert_eq!(v["installed"], false);
    assert_eq!(v["path"], env.apps.join("Peek.app").display().to_string());
    assert_eq!(v["agent"], "unknown", "launchd is never asked in tests");
    assert_eq!(v["running"]["daemon"], false);
}

#[tokio::test]
async fn login_status_reports_peekd_counts() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/api/v1/auth/me"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(common::me_body()))
        .mount(&server)
        .await;
    let env = Env::new(&server.uri());
    env.login_as("oat_c", "ort_c", unix_now() + 1800);
    let _d = fake(
        &env,
        Arc::new(|req| {
            vec![daemon::ok(
                req,
                json!({"slot": null, "drawing": null, "queue": {"pending": 0}, "pending_asks": 0,
                       "deliveries": {"pending": 2, "authority_required": 1, "last_error": null},
                       "ui_running": true}),
            )]
        }),
    );
    let run = env.run(&["login", "status", "--json"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(
        run.json()["daemon"],
        json!({"attached": true, "queued_answers": 2, "authority_required": 1})
    );
}

#[tokio::test]
async fn config_set_and_logout_reach_peekd() {
    let mut env = logged_in();
    let d = fake(
        &env,
        Arc::new(|req| match req.op.as_str() {
            "detach" => vec![daemon::ok(req, json!({"cancelled_rows": 2}))],
            _ => vec![daemon::ok(req, json!({}))],
        }),
    );
    let set = env
        .run(&[
            "config",
            "set",
            r#"{"voice":"aura-2-thalia-en","notify":["speech_finished"]}"#,
        ])
        .await;
    assert_eq!(set.code, 0, "{}", set.stderr);
    // Without the environment's opt-out, the same home syncs telemetry on.
    env.var("PEEK_TELEMETRY", "1");
    let on = env.run(&["config", "telemetry", "on", "--json"]).await;
    assert_eq!(on.code, 0, "{}", on.stderr);
    let out = env.run(&["logout", "--json"]).await;
    assert_eq!(out.code, 0, "{}", out.stderr);
    let seen = d.seen();
    let ops: Vec<&str> = seen.iter().map(|s| s.op.as_str()).collect();
    assert_eq!(ops, vec!["config.sync", "config.sync", "detach"]);
    // The tests run with PEEK_TELEMETRY=0: peekd mirrors the effective
    // setting, so that environment opt-out reaches it.
    assert_eq!(
        seen[0].fields["config"],
        json!({"voice": "aura-2-thalia-en", "language": null, "notify": ["speech_finished"], "telemetry": false,
               "env_opt_out": true})
    );
    assert_eq!(seen[1].fields["config"]["telemetry"], true);
    assert!(seen[1].fields["config"].get("env_opt_out").is_none());
    assert_eq!(seen[2].fields["reason"], "logout");
    assert!(
        seen[2].auth.is_some(),
        "detach is authenticated while the slot still exists"
    );
}

/// Builds `pkg/bin/peek` + `pkg/Peek.app.{zip,info}` around an ad-hoc signed
/// development bundle, as Honeycomb lays out a package.
fn dev_package(root: &std::path::Path) -> std::path::PathBuf {
    let run = |cmd: &str, args: &[&str]| {
        let status = std::process::Command::new(cmd)
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap_or_else(|e| panic!("{cmd}: {e}"));
        assert!(status.success(), "{cmd} {args:?} failed");
    };
    let build = root.join("build");
    let app = build.join("Peek.app");
    std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(
        app.join("Contents/Info.plist"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>ai.tos.peek.dev</string>
<key>CFBundleExecutable</key><string>Peek</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleVersion</key><string>1000</string>
<key>CFBundleShortVersionString</key><string>0.1.0</string>
</dict></plist>
"#,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    std::fs::copy("/usr/bin/true", app.join("Contents/MacOS/Peek"))
        .unwrap_or_else(|e| panic!("{e}"));
    run(
        "/usr/bin/codesign",
        &[
            "--force",
            "--deep",
            "-s",
            "-",
            app.to_str().unwrap_or_default(),
        ],
    );
    let pkg = root.join("pkg");
    std::fs::create_dir_all(pkg.join("bin")).unwrap_or_else(|e| panic!("{e}"));
    let zip = pkg.join("Peek.app.zip");
    run(
        "/usr/bin/ditto",
        &[
            "-c",
            "-k",
            "--norsrc",
            "--noextattr",
            "--noqtn",
            "--noacl",
            "--keepParent",
            app.to_str().unwrap_or_default(),
            zip.to_str().unwrap_or_default(),
        ],
    );
    let sha = {
        use sha2::Digest as _;
        hex::encode(sha2::Sha256::digest(
            std::fs::read(&zip).unwrap_or_default(),
        ))
    };
    std::fs::write(
        pkg.join("Peek.app.info"),
        format!("bundle_id=ai.tos.peek.dev\nbundle_version=1000\nshort_version=0.1.0\nteam_id=\nzip_sha256={sha}\nminimum_system_version=26.0\n"),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let peek = pkg.join("bin/peek");
    std::fs::copy(env!("CARGO_BIN_EXE_peek"), &peek).unwrap_or_else(|e| panic!("{e}"));
    peek
}

#[tokio::test]
async fn app_install_verifies_and_installs_the_bundled_app() {
    let env = Env::new(DEAD_API);
    let root = env.dir.path().canonicalize().unwrap_or_default();
    let peek = dev_package(&root);
    let run = tokio::process::Command::new(&peek)
        .args(["app", "install", "--json"])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("SILICON_HOME", &env.home)
        .env("PEEK_DAEMON_SOCKET", &env.socket)
        .env("PEEK_INSTALL_SUPPORT_DIR", &env.support)
        .env("PEEK_INSTALL_APPLICATIONS_DIR", &env.apps)
        .env("PEEK_INSTALL_NO_LAUNCH", "1")
        .env("PEEK_TELEMETRY", "0")
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let stderr = String::from_utf8_lossy(&run.stderr);
    // Installing succeeds; starting it is forbidden in tests, so the command
    // stops at launch with peek_service_unavailable (exit 5).
    assert_eq!(run.status.code(), Some(5), "{stderr}");
    assert!(stderr.contains("PEEK_INSTALL_NO_LAUNCH"), "{stderr}");
    let installed = env.apps.join("Peek.app");
    assert!(
        installed.join("Contents/Info.plist").is_file(),
        "Peek.app installed"
    );
    assert!(
        std::fs::read_dir(&env.apps).is_ok_and(|d| d.flatten().all(|e| !e
            .file_name()
            .to_string_lossy()
            .starts_with(".Peek.app.install"))),
        "the stage directory is cleaned up"
    );
    let offers: Vec<String> = std::fs::read_dir(env.support.join("offers"))
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        offers
            .iter()
            .any(|n| n.starts_with("1000-") && n.ends_with(".app.zip")),
        "{offers:?}"
    );
    assert!(
        offers
            .iter()
            .any(|n| n.starts_with("1000-") && n.ends_with(".app.info")),
        "{offers:?}"
    );
    let status =
        std::fs::read_to_string(env.support.join("install-status.txt")).unwrap_or_default();
    assert!(
        status.contains("ok\tinstall\tinstalled Peek.app 0.1.0 (build 1000)"),
        "{status}"
    );
    // A second run never touches the existing bundle.
    let again = tokio::process::Command::new(&peek)
        .args(["app", "status", "--json"])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("SILICON_HOME", &env.home)
        .env("PEEK_DAEMON_SOCKET", &env.socket)
        .env("PEEK_INSTALL_SUPPORT_DIR", &env.support)
        .env("PEEK_INSTALL_APPLICATIONS_DIR", &env.apps)
        .env("PEEK_INSTALL_NO_LAUNCH", "1")
        .env("PEEK_TELEMETRY", "0")
        .output()
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(again.status.code(), Some(0));
    let v: Value = serde_json::from_slice(&again.stdout).unwrap_or_default();
    assert_eq!(v["installed"], true);
    assert_eq!(v["build"], 1000);
    assert_eq!(v["bundle_id"], "ai.tos.peek.dev");
    assert_eq!(v["signature"], "valid");
    assert_eq!(v["best_offer"], 1000);
    assert_eq!(v["bundled"]["build"], 1000);
}

#[tokio::test]
async fn a_tampered_bundle_is_refused() {
    let env = Env::new(DEAD_API);
    let root = env.dir.path().canonicalize().unwrap_or_default();
    let peek = dev_package(&root);
    let info = root.join("pkg/Peek.app.info");
    // Corrupt the recorded checksum (still 64 hex digits).
    let fixed: String = std::fs::read_to_string(&info)
        .unwrap_or_default()
        .lines()
        .map(|l| {
            l.strip_prefix("zip_sha256=").map_or_else(
                || l.to_owned(),
                |_| format!("zip_sha256={}", "0".repeat(64)),
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&info, fixed).unwrap_or_else(|e| panic!("{e}"));
    let run = tokio::process::Command::new(&peek)
        .args(["app", "install", "--json"])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("SILICON_HOME", &env.home)
        .env("PEEK_DAEMON_SOCKET", &env.socket)
        .env("PEEK_INSTALL_SUPPORT_DIR", &env.support)
        .env("PEEK_INSTALL_APPLICATIONS_DIR", &env.apps)
        .env("PEEK_INSTALL_NO_LAUNCH", "1")
        .env("PEEK_TELEMETRY", "0")
        .output()
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert_eq!(run.status.code(), Some(5), "{stderr}");
    assert!(stderr.contains("the package is damaged"), "{stderr}");
    assert!(!env.apps.join("Peek.app").exists(), "nothing installed");
}
