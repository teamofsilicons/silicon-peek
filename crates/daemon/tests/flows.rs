//! Core flows end to end (BLUEPRINT §1.9): send with the parallel speech
//! pipeline, answers through the outbox, `--wait`, voice answers, drawings,
//! queueing and restart persistence.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::too_many_lines,
    clippy::many_single_char_names
)]

mod common;

use std::time::Duration;

use common::{DRAWING, Harness, Home, Validate, agent::ResponsePlan, eventually, pcm, query_one};
use serde_json::{Value, json};
use silicon_peek_client::{
    ErrorCode,
    ids::{AskId, SendId},
    ipc::{
        cli::{
            AskCancel, AskGet, AskList, AskState, Attach, DeliveryState, History, RegisterDrawing,
            SendOp, SendStatus, ServerSync, SpeechStatus, StatusOp, Unregister,
        },
        ui::{
            AnswerOp, Dismissed, MessageOp, Presence, PresenceReason, ShownDone, SpeechDone,
            UiAnswerVia, VoiceSubmit,
        },
    },
    runtime::daemon::EventWait,
    schema::{ask::Ask, show::Show},
    ting::Gesture,
};
use silicon_peek_daemon::stt::{tone, wav_bytes};
use wiremock::{
    Mock, ResponseTemplate,
    matchers::{header, method, path, query_param},
};

fn send_op() -> SendOp {
    SendOp {
        isi: Some("deliberate".into()),
        speak: None,
        show: None,
        ask: None,
        voice: None,
        voice_instructions: None,
        lang: None,
        notify: vec![],
        duration_ms: None,
        expires_in_s: None,
        wait: false,
        expires_at: None,
        due_at: None,
        tz: None,
        replace: false,
    }
}

fn ask(v: &Value) -> Ask {
    Ask::from_input(v).unwrap()
}

fn keep_or_delete() -> Ask {
    ask(
        &json!({"question":"Delete ~/Downloads/old.zip?","type":"single_choice",
        "options":[{"id":"keep","label":"Keep it"},{"id":"delete","label":"Delete"}]}),
    )
}

async fn ready_home(h: &Harness, actor: &str, side: u64) -> (Home, common::FakeUi) {
    let ui = h.ui().await;
    let home = h.home(actor);
    h.register(&home, side, &ui).await;
    (home, ui)
}

#[tokio::test]
async fn send_shows_the_bubble_and_streams_tts_to_the_ui() {
    let h = Harness::start().await;
    let audio = pcm(150_000);
    h.agent.audio(audio.clone());
    let (home, ui) = ready_home(&h, "si:cleanup", 3).await;
    // Sends without a side or drawing fail with the exact spec errors.
    let lonely = h.home("si:lonely");
    let mut op = send_op();
    op.speak = Some("hello".into());
    let e = h.call(&lonely, &op, vec![]).await.err().unwrap();
    assert_eq!(*e.code(), ErrorCode::SideNotRegistered);
    assert!(e.message().starts_with("no position registered for si:lonely; run `peek register side <1-8>` first (free: 1,2,4,5,6,7,8)"));

    let text = "The build finished and all tests passed. Deploying to staging now.";
    let mut op = send_op();
    op.speak = Some(text.into());
    op.show = Some(
        serde_json::from_value::<Show>(json!({"elements":[{"type":"image","path":{"blob":0},"caption":"CO2"},{"type":"text","text":"Now playing"}]}))
            .unwrap(),
    );
    let png = b"\x89PNG\r\n\x1a\nimage-bytes".to_vec();
    let (r, _) = h.call(&home, &op, vec![png.clone()]).await.unwrap();
    assert_eq!(r.status, SendStatus::Showing);
    assert_eq!(r.slot.get(), 3);
    let speech = r.speech.unwrap();
    assert_eq!(speech.status, SpeechStatus::Pending);
    assert_eq!(speech.model.as_deref(), Some("JBFqnCBsd6RMkjVDRZzb"));
    assert_eq!(speech.chars, u32::try_from(text.chars().count()).unwrap());

    let show = ui.expect("peek.show").await;
    assert_eq!(show.fields["send_id"], r.send_id.as_str());
    assert_eq!(show.fields["slot"], 3);
    assert_eq!(show.fields["context"], "production");
    assert_eq!(
        show.fields["speak"],
        json!({"text": text, "status": "pending"})
    );
    let img = show.fields["show"]["elements"][0]["path"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        img.starts_with('/')
            && std::path::Path::new(&img)
                .extension()
                .is_some_and(|x| x == "png"),
        "{img}"
    );
    assert_eq!(
        std::fs::read(&img).unwrap(),
        png,
        "the UI reads the cached copy"
    );
    assert!(show.fields["duration_ms"].as_u64().unwrap() >= 4000);

    let (begin, bytes, total) = ui.tts_stream(r.send_id.as_str()).await;
    assert_eq!(begin["format"], "s16le");
    assert_eq!(begin["sample_rate"], 24000);
    assert_eq!(begin["channels"], 1);
    assert_eq!(bytes, audio[..150_000], "every whole sample, in order");
    assert_eq!(total, 75_000);

    // The UI reports the speech and the show done; the send closes.
    ui.request(
        &SpeechDone {
            send_id: r.send_id.clone(),
            stopped_by_user: false,
            played_ms: 3125,
            total_ms: 3125,
        },
        vec![],
    )
    .await
    .unwrap();
    let payload: Vec<u8> = query_one(
        &h.db(),
        "SELECT payload FROM sends WHERE send_id = ?1",
        &[&r.send_id.as_str()],
    )
    .unwrap();
    let payload: Value = serde_json::from_slice(&payload).unwrap();
    assert_eq!(payload["speech"]["status"], "played", "the outcome is kept");
    ui.request(
        &ShownDone {
            send_id: r.send_id.clone(),
            visible_ms: 4600,
            reason: silicon_peek_client::ipc::ui::ShownReason::SpeechDone,
        },
        vec![],
    )
    .await
    .unwrap();

    // The same text again is served from the TTS cache: no second ElevenLabs call.
    let mut op = send_op();
    op.speak = Some(text.into());
    let (r2, _) = h.call(&home, &op, vec![]).await.unwrap();
    assert_eq!(r2.speech.unwrap().status, SpeechStatus::Cached);
    let show = ui.expect("peek.show").await;
    assert_eq!(show.fields["speak"]["status"], "cached");
    let (_, bytes, total) = ui.tts_stream(r2.send_id.as_str()).await;
    assert_eq!(bytes.len(), 150_000);
    assert_eq!(total, 75_000);

    let (items, _) = h
        .call(
            &home,
            &History {
                limit: Some(10),
                before: None,
            },
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(items.items.len(), 2);
    assert_eq!(items.items[1].kind, "speak+show");
    assert_eq!(items.items[1].close_reason.as_deref(), Some("speech_done"));
    assert_eq!(items.items[0].send_id, r2.send_id, "newest first");

    // A speak-only send closes on the UI's shown.done (the slide-back 1.5 s
    // after the speech), not on speech.done.
    ui.request(
        &SpeechDone {
            send_id: r2.send_id.clone(),
            stopped_by_user: false,
            played_ms: 3125,
            total_ms: 3125,
        },
        vec![],
    )
    .await
    .unwrap();
    let closed: Option<i64> = query_one(
        &h.db(),
        "SELECT closed_at FROM sends WHERE send_id = ?1",
        &[&r2.send_id.as_str()],
    )
    .unwrap();
    assert!(
        closed.is_none(),
        "speech.done alone does not close a speak-only send"
    );
    ui.request(
        &ShownDone {
            send_id: r2.send_id.clone(),
            visible_ms: 4625,
            reason: silicon_peek_client::ipc::ui::ShownReason::SpeechDone,
        },
        vec![],
    )
    .await
    .unwrap();
    let reason: Option<String> = query_one(
        &h.db(),
        "SELECT close_reason FROM sends WHERE send_id = ?1",
        &[&r2.send_id.as_str()],
    )
    .unwrap();
    assert_eq!(reason.as_deref(), Some("speech_done"));
}

#[tokio::test]
async fn multilingual_speech_and_tts_failures_fall_back_to_the_pill() {
    let h = Harness::start().await;
    h.agent.respond(vec![ResponsePlan::Reject(403)]);
    let (home, ui) = ready_home(&h, "si:cleanup", 1).await;
    let mut op = send_op();
    op.speak = Some("बिल्ड पूरा हो गया है और सभी परीक्षण सफल रहे हैं, अब हम आगे बढ़ सकते हैं।".into());
    let (r, _) = h.call(&home, &op, vec![]).await.unwrap();
    assert_eq!(r.speech.as_ref().unwrap().status, SpeechStatus::Pending);
    assert!(r.warnings.is_empty());
    let show = ui.expect("peek.show").await;
    assert_eq!(show.fields["speak"]["status"], "pending");
    let failed = ui.expect("tts.error").await;
    assert_eq!(failed.fields["send_id"], r.send_id.as_str());
    ui.request(
        &Dismissed {
            send_id: r.send_id.clone(),
            gesture: Gesture::DownArrow,
        },
        vec![],
    )
    .await
    .unwrap();

    // A non-retryable ElevenLabs failure before any audio → tts.error, one call.
    let mut op = send_op();
    op.speak = Some("The build finished and every test passed.".into());
    let (r, _) = h.call(&home, &op, vec![]).await.unwrap();
    let err = ui.expect("tts.error").await;
    assert_eq!(err.fields["send_id"], r.send_id.as_str());
    assert_eq!(err.fields["error"]["code"], "speech_unavailable");
    assert!(
        ui.try_expect("tts.begin", Duration::from_millis(200))
            .await
            .is_none()
    );
    let db = h.db();
    eventually(5, "the history warning", || {
        query_one::<String>(
            &db,
            "SELECT warnings FROM sends WHERE send_id = ?1",
            &[&r.send_id.as_str()],
        )
        .is_some_and(|w| w.contains("speech_unavailable"))
    })
    .await;
    let payload: Vec<u8> = query_one(
        &db,
        "SELECT payload FROM sends WHERE send_id = ?1",
        &[&r.send_id.as_str()],
    )
    .unwrap();
    let payload: Value = serde_json::from_slice(&payload).unwrap();
    assert_eq!(payload["speech"]["status"], "failed");
    // The warning reaches the CLI through the typed `history` reply (it
    // used to be dropped when the reply was decoded into HistoryItem).
    let (history, _) = h
        .call(
            &home,
            &History {
                limit: Some(10),
                before: None,
            },
            vec![],
        )
        .await
        .unwrap();
    let item = history
        .items
        .iter()
        .find(|i| i.send_id == r.send_id)
        .expect("the send is in history");
    assert_eq!(item.warnings.len(), 1, "{:?}", item.warnings);
    assert_eq!(item.warnings[0].code, "speech_unavailable");
    assert!(
        serde_json::to_value(&history).unwrap()["items"][0]
            .get("warnings")
            .is_some(),
        "warnings survive re-serialization (what `peek history --json` prints)"
    );
    assert_eq!(h.agent.count(), 2, "fatal provider errors are not retried");
}

#[tokio::test]
async fn a_stalled_token_mint_fails_within_the_first_audio_budget() {
    // peek-server accepts the mint but never answers in time: the §1.9.3
    // budget (3 s in tests, 20 s in production) covers minting too, so the
    // UI gets tts.error (and shows the text pill) promptly, not after the
    // 30 s request timeout.
    let h = Harness::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/speech/token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(20))
                .set_body_json(json!({
                    "access_token": "jwt-late", "expires_in": 60, "base_url": h.deepgram.uri(),
                    "key_source": "peek", "params": {"mip_opt_out": true, "tags": ["peek"]}
                })),
        )
        .with_priority(1)
        .mount(&h.server)
        .await;
    let (home, ui) = ready_home(&h, "si:cleanup", 1).await;
    let mut op = send_op();
    op.speak = Some("The build finished and every test passed.".into());
    let started = std::time::Instant::now();
    let (r, _) = h.call(&home, &op, vec![]).await.unwrap();
    let err = ui.expect("tts.error").await;
    assert_eq!(err.fields["send_id"], r.send_id.as_str());
    assert_eq!(err.fields["error"]["code"], "speech_unavailable");
    assert!(
        started.elapsed() < Duration::from_secs(7),
        "tts.error after {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn tts_retries_before_audio_but_never_after() {
    let h = Harness::start().await;
    h.agent.respond(vec![
        ResponsePlan::Reject(503),
        ResponsePlan::Audio {
            bytes: pcm(4000),
            done: true,
        },
    ]);
    let (home, ui) = ready_home(&h, "si:cleanup", 1).await;
    let mut op = send_op();
    op.speak = Some("Retry me please, the first attempt fails.".into());
    let (r, _) = h.call(&home, &op, vec![]).await.unwrap();
    let (_, bytes, total) = ui.tts_stream(r.send_id.as_str()).await;
    assert_eq!(bytes.len(), 4000);
    assert_eq!(total, 2000);
    assert_eq!(h.agent.count(), 2);
    assert!(
        ui.try_expect("tts.error", Duration::from_millis(200))
            .await
            .is_none()
    );
}

/// The delivery requests the mock server received.
async fn deliveries(h: &Harness) -> Vec<wiremock::Request> {
    h.server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.url.path() == "/api/v1/deliveries")
        .collect()
}

#[tokio::test]
async fn answers_are_delivered_with_identical_bytes_on_every_retry() {
    let h = Harness::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/deliveries"))
        .respond_with(ResponseTemplate::new(503).insert_header("retry-after", "0").set_body_json(json!({
            "error": {"code": "ting_unavailable", "message": "Ting is busy", "retryable": true, "request_id": "req_1"}
        })))
        .up_to_n_times(2)
        .mount(&h.server)
        .await;
    h.accept_deliveries().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/ting/recipient"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&h.server)
        .await;
    let (home, ui) = ready_home(&h, "si:cleanup", 2).await;
    let mut op = send_op();
    op.ask = Some(keep_or_delete());
    let (r, _) = h.call(&home, &op, vec![]).await.unwrap();
    let ask_id = r.ask_id.clone().unwrap();
    let show = ui.expect("peek.show").await;
    assert_eq!(show.fields["ask_id"], ask_id.as_str());
    assert_eq!(
        show.fields["ask"]["options"][1],
        json!({"id":"delete","label":"Delete"})
    );

    // A value that does not answer the ask is refused, and nothing is sent.
    let e = ui
        .request(
            &AnswerOp {
                send_id: r.send_id.clone(),
                ask_id: ask_id.clone(),
                value: json!("nope"),
                via: UiAnswerVia::Click,
            },
            vec![],
        )
        .await
        .err()
        .unwrap();
    assert_eq!(*e.code(), ErrorCode::InvalidInput);
    ui.request(
        &AnswerOp {
            send_id: r.send_id.clone(),
            ask_id: ask_id.clone(),
            value: json!("keep"),
            via: UiAnswerVia::Click,
        },
        vec![],
    )
    .await
    .unwrap();

    let db = h.db();
    eventually(10, "an accepted delivery", || {
        query_one::<String>(
            &db,
            "SELECT status FROM outbox WHERE subject_id = ?1",
            &[&ask_id.as_str()],
        )
        .as_deref()
            == Some("accepted")
    })
    .await;
    let reqs = deliveries(&h).await;
    assert_eq!(reqs.len(), 3, "two transient failures, then success");
    let stored: Vec<u8> = query_one(
        &db,
        "SELECT request FROM outbox WHERE subject_id = ?1",
        &[&ask_id.as_str()],
    )
    .unwrap();
    for r in &reqs {
        assert_eq!(r.body, stored, "the exact stored bytes on every attempt");
        assert_eq!(
            r.headers.get("authorization").unwrap(),
            "Bearer oat_sicleanup"
        );
        assert_eq!(r.headers.get("x-org-id").unwrap(), "tos");
    }
    let body: Value = serde_json::from_slice(&stored).unwrap();
    let event_id = body["event_id"].as_str().unwrap();
    assert_eq!(
        reqs[0].headers.get("idempotency-key").unwrap(),
        format!("peek-delivery-{event_id}").as_str()
    );
    assert_eq!(body["type"], "peek.ask.answered");
    assert_eq!(body["key"], format!("si:cleanup/{ask_id}/answered"));
    assert_eq!(
        body["data"]["answer"],
        json!({"kind":"single_choice","option_id":"keep","label":"Keep it"})
    );
    assert_eq!(body["data"]["via"], "click");
    assert_eq!(body["data"]["transcript"], json!(null));
    assert_eq!(body["data"]["question"], "Delete ~/Downloads/old.zip?");
    assert_eq!(body["data"]["slot"], 2);
    assert_eq!(body["data"]["context"], "production");
    assert_eq!(
        body["metadata"],
        json!({"isi":"deliberate","peek_version":silicon_peek_client::VERSION})
    );
    let attempts: i64 = query_one(
        &db,
        "SELECT attempts FROM outbox WHERE event_id = ?1",
        &[&event_id],
    )
    .unwrap();
    assert_eq!(attempts, 3);

    let (info, _) = h
        .call(
            &home,
            &AskGet {
                ask_id: ask_id.clone(),
            },
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(info.state, AskState::Answered);
    let delivery = info.delivery.unwrap();
    assert_eq!(delivery.status, DeliveryState::Accepted);
    assert!(delivery.ting_id.unwrap().starts_with("msg_"));
    assert_eq!(delivery.silent, Some(false));
    // Nobody else can read the ask.
    let other = h.home("si:other");
    let e = h
        .call(&other, &AskGet { ask_id }, vec![])
        .await
        .err()
        .unwrap();
    assert_eq!(*e.code(), ErrorCode::AskNotFound);
}

async fn answered_ask(h: &Harness, home: &Home, ui: &common::FakeUi) -> AskId {
    let mut op = send_op();
    op.ask = Some(keep_or_delete());
    let (r, _) = h.call(home, &op, vec![]).await.unwrap();
    let _ = ui.expect("peek.show").await;
    let ask_id = r.ask_id.unwrap();
    ui.request(
        &AnswerOp {
            send_id: r.send_id,
            ask_id: ask_id.clone(),
            value: json!("delete"),
            via: UiAnswerVia::Keyboard,
        },
        vec![],
    )
    .await
    .unwrap();
    ask_id
}

async fn outbox_row(h: &Harness, ask_id: &AskId, want: &str) -> (String, Option<String>, i64) {
    let db = h.db();
    let mut row = (String::new(), None, 0);
    eventually(10, want, || {
        row = db
            .query_row(
                "SELECT status, last_error_code, next_attempt_at FROM outbox WHERE subject_id = ?1",
                [ask_id.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap_or_default();
        row.0 == want
    })
    .await;
    row
}

#[tokio::test]
async fn terminal_and_parked_delivery_outcomes() {
    let h = Harness::start().await;
    let (home, ui) = ready_home(&h, "si:cleanup", 4).await;
    Mock::given(method("POST"))
        .and(path("/api/v1/ting/recipient"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .named("never re-register (D7)")
        .mount(&h.server)
        .await;

    // recipient_not_registered → authority_required (never re-register).
    let m = Mock::given(method("POST"))
        .and(path("/api/v1/deliveries"))
        .respond_with(ResponseTemplate::new(409).set_body_json(json!({"error":{"code":"recipient_not_registered","message":"not enrolled","retryable":false}})))
        .mount_as_scoped(&h.server)
        .await;
    let a1 = answered_ask(&h, &home, &ui).await;
    let (_, code, _) = outbox_row(&h, &a1, "authority_required").await;
    assert_eq!(code.as_deref(), Some("recipient_not_registered"));
    let (st, _) = h.call(&home, &StatusOp {}, vec![]).await.unwrap();
    assert_eq!(st.deliveries.authority_required, 1);
    // The Silicon learns at send time that answers cannot reach it.
    let mut op = send_op();
    op.show =
        Some(serde_json::from_value(json!({"elements":[{"type":"text","text":"hi"}]})).unwrap());
    let (r, _) = h.call(&home, &op, vec![]).await.unwrap();
    assert!(
        r.warnings.iter().any(|w| w.code == "ting_not_enrolled"),
        "{:?}",
        r.warnings
    );
    let _ = ui.expect("peek.show").await;
    // Always queue: the show must finish before the next ask appears.
    ui.request(
        &ShownDone {
            send_id: r.send_id.clone(),
            visible_ms: 4000,
            reason: silicon_peek_client::ipc::ui::ShownReason::Auto,
        },
        vec![],
    )
    .await
    .unwrap();
    drop(m);
    // `peek ting enroll` re-attaches the home: the parked row is retried at
    // once instead of at the next ten-minute retry.
    let accepted = Mock::given(method("POST"))
        .and(path("/api/v1/deliveries"))
        .respond_with(|req: &wiremock::Request| {
            let v: Value = serde_json::from_slice(&req.body).unwrap_or_default();
            ResponseTemplate::new(200).set_body_json(json!({
                "event_id": v["event_id"], "ting_id": "msg_1", "status": "accepted",
                "silent": false, "replayed": false
            }))
        })
        .mount_as_scoped(&h.server)
        .await;
    let started = std::time::Instant::now();
    h.call(&home, &Attach {}, vec![]).await.unwrap();
    let _ = outbox_row(&h, &a1, "accepted").await;
    assert!(started.elapsed() < Duration::from_secs(8));
    let (st, _) = h.call(&home, &StatusOp {}, vec![]).await.unwrap();
    assert_eq!(st.deliveries.authority_required, 0);
    drop(accepted);

    // ting_key_conflict → failed.
    let m = Mock::given(method("POST"))
        .and(path("/api/v1/deliveries"))
        .respond_with(ResponseTemplate::new(409).set_body_json(json!({"error":{"code":"ting_key_conflict","message":"changed body","retryable":false}})))
        .mount_as_scoped(&h.server)
        .await;
    let a2 = answered_ask(&h, &home, &ui).await;
    let (_, code, _) = outbox_row(&h, &a2, "failed").await;
    assert_eq!(code.as_deref(), Some("ting_key_conflict"));
    drop(m);

    // ting_type_missing → stays pending, retried every 15 minutes.
    let _m = Mock::given(method("POST"))
        .and(path("/api/v1/deliveries"))
        .respond_with(ResponseTemplate::new(502).set_body_json(
            json!({"error":{"code":"ting_type_missing","message":"no type","retryable":true}}),
        ))
        .mount_as_scoped(&h.server)
        .await;
    let a3 = answered_ask(&h, &home, &ui).await;
    let db = h.db();
    eventually(10, "a ting_type_missing attempt", || {
        query_one::<String>(
            &db,
            "SELECT last_error_code FROM outbox WHERE subject_id = ?1",
            &[&a3.as_str()],
        )
        .as_deref()
            == Some("ting_type_missing")
    })
    .await;
    let (status, _, next) = outbox_row(&h, &a3, "pending").await;
    assert_eq!(status, "pending");
    let now = silicon_peek_client::timestamp::Timestamp::now().unix_ms();
    assert!(next > now + 800_000, "retried in ~15 min, not sooner");
}

#[tokio::test]
async fn a_changed_testing_generation_is_rediscovered_then_delivered() {
    let h = Harness::start().await;
    let env = uuid::Uuid::parse_str("01927d6e-1c7a-7cc3-9d2e-4b4c1c1f0a11").unwrap();
    let secret = format!("ask_{}", "g".repeat(43));
    Mock::given(method("GET"))
        .and(path("/api/v1/iam"))
        .and(header("x-testing-environment-key", secret.as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "app_id": "peek", "api_version": "v1", "api_base_url": h.api().as_str(),
            "iam_base_url": "https://backend.iam.teamofsilicons.com",
            "testing_environment_id": env, "testing_generation": 8,
            "testing_environment": {"id": env, "name": "peek testing", "generation": 8},
            "compatibility": {"cli": ">=0.1.0, <1.0.0", "ipc_protocols": [1]}
        })))
        .expect(1)
        .mount(&h.server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/deliveries"))
        .and(header("x-testing-environment-generation", "7"))
        .respond_with(ResponseTemplate::new(409).set_body_json(json!({"error": {
            "code": "testing_generation_changed", "message": "now generation 8",
            "retryable": false, "details": {"generation": 8}}})))
        .expect(1)
        .mount(&h.server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/deliveries"))
        .and(header("x-testing-environment-generation", "8"))
        .and(header("x-testing-environment-key", secret.as_str()))
        .respond_with(|req: &wiremock::Request| {
            let v: Value = serde_json::from_slice(&req.body).unwrap_or_default();
            ResponseTemplate::new(200).set_body_json(json!({
                "event_id": v["event_id"], "ting_id": "msg_gen8", "status": "accepted",
                "silent": false, "replayed": false
            }))
        })
        .expect(1)
        .mount(&h.server)
        .await;
    let ui = h.ui().await;
    let home = Home::create_testing(h.root.path(), &h.api(), "si:cleanup", env, &secret, 7);
    h.register(&home, 2, &ui).await;
    let ask_id = answered_ask(&h, &home, &ui).await;
    let _ = outbox_row(&h, &ask_id, "accepted").await;
    let saved = home.store.read_testing().unwrap();
    assert_eq!(
        saved.get(env).unwrap().generation,
        8,
        "the new generation is saved"
    );
    h.server.verify().await;
}

#[tokio::test]
async fn a_live_waiter_replaces_the_ting_and_a_gone_one_does_not() {
    let h = Harness::start().await;
    h.accept_deliveries().await;
    let (home, ui) = ready_home(&h, "si:cleanup", 6).await;

    let mut conn = h.cli().await;
    let mut op = send_op();
    op.ask = Some(keep_or_delete());
    op.wait = true;
    let (r, _) = conn
        .call(&op, Some(&home.auth), vec![], Duration::from_secs(5))
        .await
        .unwrap();
    let ask_id = r.ask_id.clone().unwrap();
    let _ = ui.expect("peek.show").await;
    // The CLI reads the result and acknowledges it while the click is being
    // resolved (peekd waits for that ack before it skips the ting).
    let answer = AnswerOp {
        send_id: r.send_id.clone(),
        ask_id: ask_id.clone(),
        value: json!("keep"),
        via: UiAnswerVia::Click,
    };
    let (answered, event) = tokio::join!(ui.request(&answer, vec![]), async {
        let e = conn.next_event(Duration::from_secs(5)).await.unwrap();
        conn.acknowledge_result(&ask_id).await.unwrap();
        e
    });
    answered.unwrap();
    let EventWait::Event(e) = event else {
        panic!("expected ask.result")
    };
    assert_eq!(e.event, "ask.result");
    assert_eq!(e.fields["ask_id"], ask_id.as_str());
    assert_eq!(e.fields["state"], "answered");
    assert_eq!(
        e.fields["answer"],
        json!({"kind":"single_choice","option_id":"keep","label":"Keep it"})
    );
    assert_eq!(e.fields["via"], "click");
    let (info, _) = h
        .call(
            &home,
            &AskGet {
                ask_id: ask_id.clone(),
            },
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(info.delivery.unwrap().status, DeliveryState::Wait);
    let db = h.db();
    let rows: i64 = query_one(
        &db,
        "SELECT count(*) FROM outbox WHERE subject_id = ?1",
        &[&ask_id.as_str()],
    )
    .unwrap();
    assert_eq!(rows, 0, "no ting for an answer the waiter took");

    // The waiter leaves (timeout / ^C) → the answer goes by ting.
    let mut conn = h.cli().await;
    let (r, _) = conn
        .call(&op, Some(&home.auth), vec![], Duration::from_secs(5))
        .await
        .unwrap();
    let ask2 = r.ask_id.clone().unwrap();
    let _ = ui.expect("peek.show").await;
    drop(conn);
    eventually(5, "the waiter to detach", || {
        query_one::<i64>(
            &db,
            "SELECT waiter FROM asks WHERE ask_id = ?1",
            &[&ask2.as_str()],
        ) == Some(0)
    })
    .await;
    ui.request(
        &AnswerOp {
            send_id: r.send_id,
            ask_id: ask2.clone(),
            value: json!("delete"),
            via: UiAnswerVia::Click,
        },
        vec![],
    )
    .await
    .unwrap();
    let _ = outbox_row(&h, &ask2, "accepted").await;
    assert_eq!(deliveries(&h).await.len(), 1);

    // A dismissal reaches a waiter as its final state too.
    let mut conn = h.cli().await;
    let (r, _) = conn
        .call(&op, Some(&home.auth), vec![], Duration::from_secs(5))
        .await
        .unwrap();
    let ask3 = r.ask_id.clone().unwrap();
    let _ = ui.expect("peek.show").await;
    let dismiss = Dismissed {
        send_id: r.send_id,
        gesture: Gesture::Esc,
    };
    let (dismissed, event) = tokio::join!(ui.request(&dismiss, vec![]), async {
        let e = conn.next_event(Duration::from_secs(5)).await.unwrap();
        conn.acknowledge_result(&ask3).await.unwrap();
        e
    });
    dismissed.unwrap();
    let EventWait::Event(e) = event else {
        panic!("expected ask.result")
    };
    assert_eq!(e.fields["state"], "dismissed");

    // A CLI that received the result but never acknowledged it (its --wait
    // timer fired as the answer arrived, and it exited) did not deliver it:
    // the answer goes by ting.
    let mut conn = h.cli().await;
    let (r, _) = conn
        .call(&op, Some(&home.auth), vec![], Duration::from_secs(5))
        .await
        .unwrap();
    let ask4 = r.ask_id.clone().unwrap();
    let _ = ui.expect("peek.show").await;
    let answer = AnswerOp {
        send_id: r.send_id.clone(),
        ask_id: ask4.clone(),
        value: json!("keep"),
        via: UiAnswerVia::Click,
    };
    let (answered, event) = tokio::join!(
        ui.request(&answer, vec![]),
        conn.next_event(Duration::from_secs(5))
    );
    answered.unwrap();
    assert!(
        matches!(event, Ok(EventWait::Event(_))),
        "the result was written"
    );
    let _ = outbox_row(&h, &ask4, "accepted").await;
    let (info, _) = h
        .call(
            &home,
            &AskGet {
                ask_id: ask4.clone(),
            },
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(info.delivery.unwrap().status, DeliveryState::Accepted);
    drop(conn);
}

#[tokio::test]
async fn voice_answers_are_transcribed_once_and_matched() {
    let h = Harness::start().await;
    h.accept_deliveries().await;
    let listen = Mock::given(method("POST"))
        .and(path("/api/v1/speech/listen"))
        .and(query_param("model", "gpt-transcribe"))
        .and(query_param("keyterm", "Keep it"))
        .and(query_param("language", "en"))
        .and(header("content-type", "audio/wav"))
        .and(header("authorization", "Bearer oat_sicleanup"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "request_id": "stt-1", "detected_language": "en", "text": " The second one. "
        })))
        .expect(1)
        .mount_as_scoped(&h.server)
        .await;
    let (home, ui) = ready_home(&h, "si:cleanup", 7).await;
    let mut op = send_op();
    op.ask = Some(keep_or_delete());
    let (r, _) = h.call(&home, &op, vec![]).await.unwrap();
    let ask_id = r.ask_id.clone().unwrap();
    let _ = ui.expect("peek.show").await;
    let wav = wav_bytes(&tone(1500, 8000), 16_000);
    let reply = ui
        .request(
            &VoiceSubmit {
                send_id: Some(r.send_id.clone()),
                ask_id: Some(ask_id.clone()),
                slot: r.slot,
                duration_ms: 1500,
                languages: vec!["en-US".into()],
                context: None,
            },
            vec![wav.clone()],
        )
        .await
        .unwrap();
    assert_eq!(reply.message_id, None, "a voice answer names no message");
    let res = ui.expect("stt.result").await;
    assert_eq!(res.fields["ask_id"], ask_id.as_str());
    assert_eq!(res.fields["outcome"], "matched");
    assert_eq!(res.fields["value"], "delete");
    let _ = outbox_row(&h, &ask_id, "accepted").await;
    let body: Value = serde_json::from_slice(&deliveries(&h).await[0].body).unwrap();
    assert_eq!(body["data"]["via"], "voice");
    assert_eq!(body["data"]["transcript"], "The second one.");
    assert_eq!(body["data"]["answer"]["option_id"], "delete");
    let received = h.server.received_requests().await.unwrap();
    assert_eq!(
        received
            .iter()
            .find(|r| r.url.path() == "/api/v1/speech/listen")
            .unwrap()
            .body,
        wav
    );
    assert!(
        !h.cfg
            .support_dir
            .join(format!("recordings/{ask_id}.wav"))
            .exists(),
        "deleted once delivered"
    );
    drop(listen);

    // Unmatched: the ask stays open and nothing is sent.
    Mock::given(method("POST"))
        .and(path("/api/v1/speech/listen"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "text": "what about pizza"
        })))
        .mount(&h.server)
        .await;
    let (r, _) = h.call(&home, &op, vec![]).await.unwrap();
    let ask2 = r.ask_id.clone().unwrap();
    let _ = ui.expect("peek.show").await;
    let submit = VoiceSubmit {
        send_id: Some(r.send_id.clone()),
        ask_id: Some(ask2.clone()),
        slot: r.slot,
        duration_ms: 1500,
        languages: vec![],
        context: None,
    };
    ui.request(&submit, vec![wav.clone()]).await.unwrap();
    let res = ui.expect("stt.result").await;
    assert_eq!(res.fields["outcome"], "unmatched");
    let (info, _) = h
        .call(
            &home,
            &AskGet {
                ask_id: ask2.clone(),
            },
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(info.state, AskState::Pending);

    // Silence: nothing is uploaded, the outcome is `empty`.
    let before = h
        .server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == "/api/v1/speech/listen")
        .count();
    ui.request(&submit, vec![wav_bytes(&vec![0; 16_000], 16_000)])
        .await
        .unwrap();
    let res = ui.expect("stt.result").await;
    assert_eq!(res.fields["outcome"], "empty");
    assert_eq!(
        h.server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.url.path() == "/api/v1/speech/listen")
            .count(),
        before
    );

    // A broken WAV is refused up front.
    let e = ui
        .request(&submit, vec![b"not a wav".to_vec()])
        .await
        .err()
        .unwrap();
    assert_eq!(*e.code(), ErrorCode::InvalidInput);
}

#[tokio::test]
async fn a_voice_message_reply_names_the_message_before_its_stt_result() {
    let h = Harness::start().await;
    let (_home, ui) = ready_home(&h, "si:cleanup", 4).await;
    ui.arrivals.lock().unwrap().clear();
    // Silence: the outcome is known at once (no upload), which is exactly
    // when the `stt.result` could overtake the reply if peekd started
    // transcribing before replying.
    let reply = ui
        .request(
            &VoiceSubmit {
                send_id: None,
                ask_id: None,
                slot: silicon_peek_client::identity::SlotIndex::new(4).unwrap(),
                duration_ms: 1000,
                languages: vec![],
                context: None,
            },
            vec![wav_bytes(&vec![0; 16_000], 16_000)],
        )
        .await
        .unwrap();
    let message_id = reply.message_id.expect("a voice message is named");
    assert!(message_id.as_str().starts_with("cmsg_"), "{message_id}");
    let res = ui.expect("stt.result").await;
    assert_eq!(res.fields["message_id"], message_id.as_str());
    assert!(res.fields["ask_id"].is_null());
    assert_eq!(res.fields["outcome"], "empty");
    let order: Vec<String> = ui
        .arrivals
        .lock()
        .unwrap()
        .iter()
        .filter(|a| *a == "reply" || *a == "event:stt.result")
        .cloned()
        .collect();
    assert_eq!(order, vec!["reply", "event:stt.result"]);
}

#[tokio::test]
async fn voice_on_a_slider_uses_numerals_and_a_message_without_an_ask() {
    let h = Harness::start().await;
    h.accept_deliveries().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/speech/listen"))
        .and(query_param("numerals", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "text": "Set it to 42%."
        })))
        .expect(1)
        .mount(&h.server)
        .await;
    let (home, ui) = ready_home(&h, "si:dj-bot", 8).await;
    let mut op = send_op();
    op.ask = Some(ask(
        &json!({"question":"Volume?","type":"slider","min":0,"max":100}),
    ));
    let (r, _) = h.call(&home, &op, vec![]).await.unwrap();
    let ask_id = r.ask_id.clone().unwrap();
    let _ = ui.expect("peek.show").await;
    ui.request(
        &VoiceSubmit {
            send_id: Some(r.send_id.clone()),
            ask_id: Some(ask_id.clone()),
            slot: r.slot,
            duration_ms: 900,
            languages: vec![],
            context: None,
        },
        vec![wav_bytes(&tone(900, 9000), 16_000)],
    )
    .await
    .unwrap();
    let res = ui.expect("stt.result").await;
    assert_eq!(res.fields["value"], 42);
    let (info, _) = h.call(&home, &AskGet { ask_id }, vec![]).await.unwrap();
    assert_eq!(
        serde_json::to_value(info.answer).unwrap(),
        json!({"kind":"slider","value":42})
    );

    // A typed Carbon message with no pending ask.
    let reply = ui
        .request(
            &MessageOp {
                slot: r.slot,
                text: "  remind me about this at 5  ".into(),
                via: silicon_peek_client::ting::MessageVia::Keyboard,
                context: None,
            },
            vec![],
        )
        .await
        .unwrap();
    let db = h.db();
    eventually(10, "the message delivery", || {
        query_one::<String>(
            &db,
            "SELECT status FROM outbox WHERE subject_id = ?1",
            &[&reply.message_id.as_str()],
        )
        .as_deref()
            == Some("accepted")
    })
    .await;
    let delivered = deliveries(&h).await;
    let body: Value = serde_json::from_slice(&delivered.last().unwrap().body).unwrap();
    assert_eq!(body["type"], "peek.message.received");
    assert_eq!(body["data"]["text"], "remind me about this at 5");
    assert_eq!(body["data"]["via"], "keyboard");
    assert_eq!(body["data"]["in_reply_to"], r.send_id.as_str());
    assert_eq!(
        body["metadata"]["isi"], "deliberate",
        "the ISI of the slot's most recent send"
    );
    let e = ui
        .request(
            &MessageOp {
                slot: silicon_peek_client::identity::SlotIndex::new(1).unwrap(),
                text: "hi".into(),
                via: silicon_peek_client::ting::MessageVia::Keyboard,
                context: None,
            },
            vec![],
        )
        .await
        .err()
        .unwrap();
    assert_eq!(*e.code(), ErrorCode::SideNotRegistered);
}

#[tokio::test]
async fn drawings_validate_in_the_ui_activate_and_sync() {
    let h = Harness::start().await;
    let ui = h.ui().await;
    let home = h.home("si:artist");
    h.call(
        &home,
        &silicon_peek_client::ipc::cli::RegisterSide {
            index: silicon_peek_client::identity::SlotIndex::new(2).unwrap(),
        },
        vec![],
    )
    .await
    .unwrap();
    let reg = |check_only: bool, preview: bool| RegisterDrawing {
        filename: "logo.js".into(),
        check_only,
        preview,
        dump_frame: None,
    };
    // --check validates only.
    let (r, blobs) = h
        .call(&home, &reg(true, true), vec![DRAWING.to_vec()])
        .await
        .unwrap();
    assert!(!r.active);
    assert_eq!(r.server_sync, ServerSync::Skipped);
    assert_eq!(
        blobs,
        vec![b"\x89PNG\r\n\x1a\npreview".to_vec()],
        "the preview PNG comes back"
    );
    assert_eq!(ui.validated_scripts.lock().unwrap()[0], DRAWING);

    // Register for real.
    let (r, blobs) = h
        .call(&home, &reg(false, false), vec![DRAWING.to_vec()])
        .await
        .unwrap();
    assert!(r.active);
    assert!(blobs.is_empty());
    assert_eq!(r.server_sync, ServerSync::Pending);
    assert_eq!(
        r.slot.map(silicon_peek_client::identity::SlotIndex::get),
        Some(2)
    );
    assert_eq!(r.stats.frames, 90);
    assert_eq!(r.logs, vec![json!("hello from the drawing")]);
    assert_eq!(r.bytes, u64::try_from(DRAWING.len()).unwrap());
    let path = h
        .cfg
        .support_dir
        .join(format!("drawings/production/tos/si:artist/{}.js", r.sha256));
    assert_eq!(std::fs::read(&path).unwrap(), DRAWING);
    eventually(5, "drawing.load", || {
        ui.requests
            .lock()
            .unwrap()
            .iter()
            .any(|(op, _)| op == "drawing.load")
    })
    .await;
    let load = ui
        .requests
        .lock()
        .unwrap()
        .iter()
        .find(|(op, _)| op == "drawing.load")
        .unwrap()
        .1
        .clone();
    assert_eq!(load["sha256"], r.sha256.as_str());
    assert_eq!(load["slot"], 2);
    assert_eq!(load["actor_id"], "si:artist");

    // The outbox uploads the exact script.
    let db = h.db();
    eventually(10, "the server copy", || {
        query_one::<String>(&db, "SELECT server_sync FROM drawings", &[]).as_deref()
            == Some("synced")
    })
    .await;
    let put = h
        .server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.method.as_str() == "PUT")
        .unwrap();
    assert_eq!(put.body, DRAWING);
    assert_eq!(
        put.headers.get("x-peek-drawing-sha256").unwrap(),
        r.sha256.as_str()
    );
    assert_eq!(
        put.headers.get("content-type").unwrap(),
        "application/javascript"
    );

    // A failing drawing keeps the previous one active.
    *ui.validate.lock().unwrap() = Validate::Fail("ReferenceError: foo is not defined".into());
    let broken = b"export default function draw() { foo(); }".to_vec();
    let e = h
        .call(&home, &reg(false, false), vec![broken])
        .await
        .err()
        .unwrap();
    assert_eq!(*e.code(), ErrorCode::DrawingInvalid);
    assert_eq!(e.exit_code().code(), 4);
    assert!(
        e.message()
            .contains("at test frame 12: ReferenceError: foo is not defined")
    );
    assert_eq!(
        e.details().unwrap()["error"]["stack"],
        "at draw (logo.js:1:10)"
    );
    let (st, _) = h.call(&home, &StatusOp {}, vec![]).await.unwrap();
    // The drawing's backend copy is not a Ting delivery.
    assert_eq!(st.deliveries.pending, 0);
    assert_eq!(st.deliveries.authority_required, 0);
    let drawing = st.drawing.unwrap();
    assert!(
        matches!(drawing.server_sync.as_deref(), Some("synced" | "pending")),
        "{drawing:?}"
    );
    assert_eq!(drawing.sha256, r.sha256);

    // Too large is refused before the UI is asked.
    let e = h
        .call(&home, &reg(false, false), vec![vec![b' '; 256 * 1024 + 1]])
        .await
        .err()
        .unwrap();
    assert_eq!(*e.code(), ErrorCode::FrameTooLarge);

    // A runtime failure is attached once to the next CLI result.
    ui.request(
        &silicon_peek_client::ipc::ui::DrawingError {
            context: silicon_peek_client::identity::Context::Production,
            org_id: silicon_peek_client::identity::OrgId::parse("tos").unwrap(),
            actor_id: silicon_peek_client::identity::ActorId::parse("si:artist").unwrap(),
            reason: silicon_peek_client::ipc::ui::DrawingFailure::Throws,
            message: "TypeError: x is undefined".into(),
            stack: Some("at draw".into()),
        },
        vec![],
    )
    .await
    .unwrap();
    let (st, _) = h.call(&home, &StatusOp {}, vec![]).await.unwrap();
    assert_eq!(st.warnings[0].code, "drawing_fallback_active");
    assert!(!st.drawing.as_ref().unwrap().active);
    assert_eq!(
        st.drawing.unwrap().last_error.as_deref(),
        Some("TypeError: x is undefined")
    );
    let (st, _) = h.call(&home, &StatusOp {}, vec![]).await.unwrap();
    assert!(st.warnings.is_empty(), "attached once");

    // Unregister releases the slot, deletes the drawing locally and remotely.
    let (u, _) = h.call(&home, &Unregister {}, vec![]).await.unwrap();
    assert_eq!(
        u.released_slot
            .map(silicon_peek_client::identity::SlotIndex::get),
        Some(2)
    );
    assert!(!path.exists());
    eventually(10, "the server delete", || {
        query_one::<String>(
            &db,
            "SELECT status FROM outbox WHERE kind = 'drawing.delete'",
            &[],
        )
        .as_deref()
            == Some("accepted")
    })
    .await;
    let mut op = send_op();
    op.speak = Some("hi".into());
    let e = h.call(&home, &op, vec![]).await.err().unwrap();
    assert_eq!(*e.code(), ErrorCode::SideNotRegistered);
}

#[tokio::test]
async fn sends_queue_behind_a_pending_ask_up_to_five() {
    let h = Harness::start().await;
    let (home, ui) = ready_home(&h, "si:cleanup", 5).await;
    let mut op = send_op();
    op.ask = Some(keep_or_delete());
    let (first, _) = h.call(&home, &op, vec![]).await.unwrap();
    assert_eq!(first.status, SendStatus::Showing);
    assert_eq!(first.queue_position, Some(0));
    assert_eq!(first.waiting, Some(0));
    let _ = ui.expect("peek.show").await;
    let mut queued = Vec::new();
    for i in 0..5 {
        let mut s = send_op();
        s.show = Some(
            serde_json::from_value(
                json!({"elements":[{"type":"text","text":format!("note {i}")}]}),
            )
            .unwrap(),
        );
        let (r, _) = h.call(&home, &s, vec![]).await.unwrap();
        assert_eq!(r.status, SendStatus::Queued);
        assert_eq!(r.queue_position, Some(i + 1));
        assert_eq!(r.waiting, Some(i + 1));
        let badge = ui.expect("queue.state").await;
        assert_eq!(badge.fields["send_id"], first.send_id.as_str());
        assert_eq!(badge.fields["waiting"], i + 1);
        assert_eq!(badge.fields["slot"], 5);
        queued.push(r.send_id);
    }
    let mut s = send_op();
    s.speak = Some("one too many".into());
    let e = h.call(&home, &s, vec![]).await.err().unwrap();
    assert_eq!(*e.code(), ErrorCode::QueueFull);
    assert_eq!(e.exit_code().code(), 4);
    assert!(e.retryable());
    assert_eq!(
        e.message(),
        "position 5's queue is full: 1 send on screen and 5 waiting (at most 5); remove one with `peek cancel <send_id>` or `peek queue clear`"
    );
    assert_eq!(
        e.hint(),
        Some("peek queue    lists the waiting sends and their IDs")
    );
    let d = e.details().unwrap();
    assert_eq!(d["queued"], 5);
    assert_eq!(d["limit"], 5);
    assert_eq!(d["on_screen"], first.send_id.as_str());
    assert_eq!(
        d["waiting"],
        json!(queued.iter().map(SendId::as_str).collect::<Vec<_>>())
    );
    assert_eq!(d["due_waiting"], 0);
    assert_eq!(d["held"], Value::Null);
    // A CLI older than 0.1.2 gets the same error as `slot_busy`.
    let mut legacy = common::DaemonCli::legacy(&h, "0.1.1").await;
    let e = legacy.call(&home, &s).await.err().unwrap();
    assert_eq!(*e.code(), ErrorCode::SlotBusy);
    assert!(e.message().starts_with("position 5's queue is full"));
    assert_eq!(e.details().unwrap()["queued"], 5);
    let (st, _) = h.call(&home, &StatusOp {}, vec![]).await.unwrap();
    assert_eq!(st.queue.pending, 5);
    assert_eq!(st.queue.waiting, 5);
    assert_eq!(st.queue.limit, 5);
    assert_eq!(st.queue.on_screen.as_ref(), Some(&first.send_id));
    assert_eq!(st.queue.held, None);
    assert_eq!(st.pending_asks, 1);

    // Cancelling the ask withdraws it and shows the next queued send.
    let (c, _) = h
        .call(
            &home,
            &AskCancel {
                ask_id: first.ask_id.clone().unwrap(),
            },
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(c.state, AskState::Cancelled);
    let cancel = ui.expect("peek.cancel").await;
    assert_eq!(cancel.fields["send_id"], first.send_id.as_str());
    assert_eq!(cancel.fields["reason"], "cancelled_by_silicon");
    let next = ui.expect("peek.show").await;
    assert_eq!(next.fields["send_id"], queued[0].as_str());
    assert_eq!(next.fields["queued_behind"], 4);
    // Cancelling again reports the final state.
    let (c, _) = h
        .call(
            &home,
            &AskCancel {
                ask_id: first.ask_id.clone().unwrap(),
            },
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(c.state, AskState::Cancelled);
    // A dismissed show makes way for the next one.
    ui.request(
        &Dismissed {
            send_id: queued[0].clone(),
            gesture: Gesture::DownArrow,
        },
        vec![],
    )
    .await
    .unwrap();
    let next = ui.expect("peek.show").await;
    assert_eq!(next.fields["send_id"], queued[1].as_str());
    assert_eq!(next.fields["queued_behind"], 3);
    let (lst, _) = h
        .call(
            &home,
            &AskList {
                state: Some(AskState::Cancelled),
                limit: None,
            },
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(lst.asks.len(), 1);
}

#[tokio::test]
async fn a_locked_screen_holds_bubbles_until_the_carbon_is_back() {
    let h = Harness::start().await;
    let (home, ui) = ready_home(&h, "si:cleanup", 2).await;
    let show = |t: &str| {
        let mut s = send_op();
        s.show =
            Some(serde_json::from_value(json!({"elements":[{"type":"text","text":t}]})).unwrap());
        s
    };
    ui.request(
        &Presence {
            available: false,
            reason: PresenceReason::Locked,
            paused: false,
        },
        vec![],
    )
    .await
    .unwrap();
    let (st, _) = h.call(&home, &StatusOp {}, vec![]).await.unwrap();
    let carbon = st.carbon.unwrap();
    assert!(!carbon.available);
    assert_eq!(carbon.reason, PresenceReason::Locked);

    // Nothing is shown to nobody: every send waits, none replaces another.
    let (first_send, _) = h.call(&home, &show("first"), vec![]).await.unwrap();
    let (second_send, _) = h.call(&home, &show("second"), vec![]).await.unwrap();
    let mut op = send_op();
    op.ask = Some(keep_or_delete());
    op.expires_in_s = Some(600);
    let (ask_send, _) = h.call(&home, &op, vec![]).await.unwrap();
    for r in [&first_send, &second_send, &ask_send] {
        assert_eq!(r.status, SendStatus::Queued);
        let w = r.warnings.iter().find(|w| w.code == "carbon_away").unwrap();
        assert_eq!(w.details.as_ref().unwrap()["reason"], "locked");
    }
    assert!(
        ui.try_expect("peek.show", Duration::from_millis(300))
            .await
            .is_none()
    );
    let db = h.db();
    let shown: Option<i64> = query_one(
        &db,
        "SELECT shown_at FROM sends WHERE send_id = ?1",
        &[&first_send.send_id.as_str()],
    );
    assert!(shown.is_none(), "never recorded as shown while away");
    let warnings: String = query_one(
        &db,
        "SELECT warnings FROM sends WHERE send_id = ?1",
        &[&first_send.send_id.as_str()],
    )
    .unwrap();
    assert!(
        warnings.contains("carbon_away"),
        "history keeps the warning"
    );
    // An ask's expiry clock keeps running while it waits.
    let expires: Option<i64> = query_one(
        &db,
        "SELECT expires_at FROM asks WHERE send_id = ?1",
        &[&ask_send.send_id.as_str()],
    );
    assert!(expires.is_some());

    // Back: the held sends are shown in order.
    ui.request(&Presence::default(), vec![]).await.unwrap();
    let first = ui.expect("peek.show").await;
    assert_eq!(first.fields["send_id"], first_send.send_id.as_str());
    assert_eq!(first.fields["queued_behind"], 2);
    ui.request(
        &ShownDone {
            send_id: first_send.send_id.clone(),
            visible_ms: 4000,
            reason: silicon_peek_client::ipc::ui::ShownReason::Auto,
        },
        vec![],
    )
    .await
    .unwrap();
    let second = ui.expect("peek.show").await;
    assert_eq!(second.fields["send_id"], second_send.send_id.as_str());
    let (st, _) = h.call(&home, &StatusOp {}, vec![]).await.unwrap();
    assert!(st.carbon.unwrap().available);
    // Available again, a new show replaces the visible one as before.
    let (later, _) = h.call(&home, &show("third"), vec![]).await.unwrap();
    assert_eq!(
        later.status,
        SendStatus::Queued,
        "the ask still waits in line"
    );
    assert!(later.warnings.iter().all(|w| w.code != "carbon_away"));
}

#[tokio::test]
async fn a_send_warns_when_the_session_has_no_ting_enrollment() {
    let h = Harness::start().await;
    let (home, ui) = ready_home(&h, "si:cleanup", 6).await;
    home.edit_session(|f| {
        for slot in f.slots.values_mut() {
            slot.ting = Some(silicon_peek_client::runtime::session::SlotTing {
                subscribed: false,
                subscription_id: None,
                registered_at: None,
                error: None,
            });
        }
    });
    let mut op = send_op();
    op.ask = Some(keep_or_delete());
    let (r, _) = h.call(&home, &op, vec![]).await.unwrap();
    let w = r
        .warnings
        .iter()
        .find(|w| w.code == "ting_not_enrolled")
        .expect("ting_not_enrolled");
    assert_eq!(w.details.as_ref().unwrap()["next"], "peek ting enroll");
    let _ = ui.expect("peek.show").await;
}

#[tokio::test]
async fn a_new_show_waits_behind_a_visible_show() {
    let h = Harness::start().await;
    let (home, ui) = ready_home(&h, "si:dj-bot", 1).await;
    let show = |t: &str| {
        let mut s = send_op();
        s.show =
            Some(serde_json::from_value(json!({"elements":[{"type":"text","text":t}]})).unwrap());
        s
    };
    let (a, _) = h
        .call(&home, &show("Now playing: CO2"), vec![])
        .await
        .unwrap();
    let (b, _) = h
        .call(&home, &show("Now playing: Kasoor"), vec![])
        .await
        .unwrap();
    let (c, _) = h
        .call(&home, &show("Now playing: Tu Hai Kahan"), vec![])
        .await
        .unwrap();
    assert_eq!(a.status, SendStatus::Showing);
    assert_eq!(
        b.status,
        SendStatus::Queued,
        "always queue: nothing is replaced"
    );
    assert_eq!((b.queue_position, b.waiting), (Some(1), Some(1)));
    assert_eq!((c.queue_position, c.waiting), (Some(2), Some(2)));
    let first = ui.expect("peek.show").await;
    assert_eq!(first.fields["send_id"], a.send_id.as_str());
    assert!(
        ui.try_expect("peek.show", Duration::from_millis(300))
            .await
            .is_none(),
        "the second show waits for the first"
    );
    let closed: Option<i64> = query_one(
        &h.db(),
        "SELECT closed_at FROM sends WHERE send_id = ?1",
        &[&a.send_id.as_str()],
    );
    assert!(closed.is_none(), "the first show is still current");
    // Strict FIFO: B, then C.
    ui.request(
        &ShownDone {
            send_id: a.send_id.clone(),
            visible_ms: 4000,
            reason: silicon_peek_client::ipc::ui::ShownReason::Auto,
        },
        vec![],
    )
    .await
    .unwrap();
    let second = ui.expect("peek.show").await;
    assert_eq!(second.fields["send_id"], b.send_id.as_str());
    assert_eq!(second.fields["queued_behind"], 1);
    ui.request(
        &ShownDone {
            send_id: b.send_id.clone(),
            visible_ms: 4000,
            reason: silicon_peek_client::ipc::ui::ShownReason::Auto,
        },
        vec![],
    )
    .await
    .unwrap();
    let third = ui.expect("peek.show").await;
    assert_eq!(third.fields["send_id"], c.send_id.as_str());
    assert_eq!(third.fields["queued_behind"], 0);
}

#[tokio::test]
async fn state_survives_a_restart() {
    let mut h = Harness::start().await;
    let (home, ui) = ready_home(&h, "si:cleanup", 3).await;
    ui.close();
    common::eventually(5, "the UI to go", || !h.handle().ui_connected()).await;
    let mut op = send_op();
    op.ask = Some(keep_or_delete());
    op.expires_in_s = Some(3600);
    let (asked, _) = h.call(&home, &op, vec![]).await.unwrap();
    assert_eq!(
        asked.status,
        SendStatus::Queued,
        "held: shown when Peek.app connects"
    );
    assert_eq!(asked.queue_position, Some(0));
    let mut s = send_op();
    s.speak = Some("queued behind the ask".into());
    let (queued, _) = h.call(&home, &s, vec![]).await.unwrap();
    assert_eq!(queued.status, SendStatus::Queued);

    h.restart().await;

    let (st, _) = h.call(&home, &StatusOp {}, vec![]).await.unwrap();
    assert_eq!(st.slot.map(|s| s.index.get()), Some(3));
    assert!(st.drawing.is_some());
    assert_eq!(st.pending_asks, 1);
    assert_eq!(st.queue.pending, 1);
    let ui = h.ui().await;
    let state = ui.expect("slots.state").await;
    assert_eq!(state.fields["slots"][0]["actor_id"], "si:cleanup");
    assert!(
        std::path::Path::new(
            state.fields["slots"][0]["drawing"]["path"]
                .as_str()
                .unwrap()
        )
        .extension()
        .is_some_and(|x| x == "js")
    );
    let show = ui.expect("peek.show").await;
    assert_eq!(show.fields["send_id"], asked.send_id.as_str());
    assert_eq!(
        show.fields["ask_id"],
        asked.ask_id.clone().unwrap().as_str()
    );
    assert!(show.fields["expires_at"].as_str().is_some());
    assert_eq!(show.fields["queued_behind"], 1);
    let (info, _) = h
        .call(
            &home,
            &AskGet {
                ask_id: asked.ask_id.unwrap(),
            },
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(info.state, AskState::Pending);
    let _ = SendId::generate();
}

#[tokio::test]
async fn asks_expire_and_notify_the_silicon() {
    let h = Harness::start().await;
    h.accept_deliveries().await;
    let (home, ui) = ready_home(&h, "si:cleanup", 2).await;
    let mut op = send_op();
    op.ask = Some(keep_or_delete());
    op.expires_in_s = Some(10);
    let (r, _) = h.call(&home, &op, vec![]).await.unwrap();
    let ask_id = r.ask_id.clone().unwrap();
    let _ = ui.expect("peek.show").await;
    // Fast-forward: the ask is already past its deadline; any new expiring
    // ask wakes the timer.
    h.db()
        .execute(
            "UPDATE asks SET expires_at = 1 WHERE ask_id = ?1",
            [ask_id.as_str()],
        )
        .unwrap();
    let other = h.home("si:other");
    h.register(&other, 4, &ui).await;
    h.call(&other, &op, vec![]).await.unwrap();
    let cancel = ui.expect("peek.cancel").await;
    assert_eq!(cancel.fields["send_id"], r.send_id.as_str());
    assert_eq!(cancel.fields["reason"], "expired");
    let _ = outbox_row(&h, &ask_id, "accepted").await;
    let body: Value = serde_json::from_slice(
        &query_one::<Vec<u8>>(
            &h.db(),
            "SELECT request FROM outbox WHERE subject_id = ?1",
            &[&ask_id.as_str()],
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(body["type"], "peek.ask.expired");
    assert_eq!(body["key"], format!("si:cleanup/{ask_id}/expired"));
}

#[tokio::test]
async fn logout_detach_cancels_undelivered_rows() {
    use silicon_peek_client::ipc::cli::{Detach, DetachReason};
    let h = Harness::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/deliveries"))
        .respond_with(ResponseTemplate::new(503).set_body_json(
            json!({"error":{"code":"ting_unavailable","message":"down","retryable":true}}),
        ))
        .mount(&h.server)
        .await;
    let (home, ui) = ready_home(&h, "si:cleanup", 2).await;
    let a = answered_ask(&h, &home, &ui).await;
    let db = h.db();
    eventually(5, "a failed attempt", || {
        query_one::<i64>(
            &db,
            "SELECT attempts FROM outbox WHERE subject_id = ?1",
            &[&a.as_str()],
        )
        .unwrap_or(0)
            >= 1
    })
    .await;
    // `peek logout`: the tombstone is written and the slot deleted first.
    let key =
        silicon_peek_client::identity::SlotKey::new(home.auth.api_url.clone(), home.auth.context);
    home.edit_session(|s| {
        s.begin_logout(&key, 1);
        s.slots.clear();
    });
    let (d, _) = h
        .call(
            &home,
            &Detach {
                reason: DetachReason::Logout,
            },
            vec![],
        )
        .await
        .unwrap();
    // Either detach cancels the row, or the worker already did on seeing
    // the logout tombstone; both leave it cancelled as `logged_out`.
    assert!(d.cancelled_rows <= 1);
    let status: String = query_one(
        &db,
        "SELECT status FROM outbox WHERE subject_id = ?1",
        &[&a.as_str()],
    )
    .unwrap();
    assert_eq!(status, "cancelled");
    let code: String = query_one(
        &db,
        "SELECT last_error_code FROM outbox WHERE subject_id = ?1",
        &[&a.as_str()],
    )
    .unwrap();
    assert_eq!(code, "logged_out");
    let e = h.call(&home, &StatusOp {}, vec![]).await.err().unwrap();
    assert_eq!(*e.code(), ErrorCode::NotLoggedIn);
}

#[tokio::test]
async fn unregister_wins_over_an_in_flight_upload_and_a_later_register_side() {
    use sha2::{Digest, Sha256};
    let h = Harness::start().await;
    // The upload is still in flight when the Silicon unregisters, then fails
    // with a retryable 503; the delete keeps failing too (stays pending).
    Mock::given(method("PUT"))
        .and(path("/api/v1/drawings/current"))
        .respond_with(
            ResponseTemplate::new(503)
                .set_delay(Duration::from_millis(1500))
                .set_body_json(json!({"error": {"code": "backend_unavailable", "message": "busy", "retryable": true}})),
        )
        .with_priority(1)
        .mount(&h.server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/v1/drawings/current"))
        .respond_with(ResponseTemplate::new(503).set_body_json(
            json!({"error": {"code": "backend_unavailable", "message": "busy", "retryable": true}}),
        ))
        .with_priority(1)
        .mount(&h.server)
        .await;
    let ui = h.ui().await;
    let home = h.home("si:artist");
    let side = || silicon_peek_client::ipc::cli::RegisterSide {
        index: silicon_peek_client::identity::SlotIndex::new(3).unwrap(),
    };
    h.call(&home, &side(), vec![]).await.unwrap();
    let (r, _) = h
        .call(
            &home,
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
    assert!(r.active);
    let puts = || async {
        h.server
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .filter(|r| r.method.as_str() == "PUT")
            .count()
    };
    for _ in 0..100 {
        if puts().await > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(puts().await, 1, "the upload is in flight");
    // From here the server holds a copy of the drawing until the delete
    // lands (it must never come back).
    let sha = hex::encode(Sha256::digest(DRAWING));
    Mock::given(method("GET"))
        .and(path("/api/v1/drawings/current"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", format!("\"{sha}\"").as_str())
                .set_body_bytes(DRAWING.to_vec()),
        )
        .with_priority(1)
        .mount(&h.server)
        .await;
    h.call(&home, &Unregister {}, vec![]).await.unwrap();
    // The in-flight attempt ends with a 503 (retry) after the unregister
    // cancelled it: it stays cancelled and is never sent again.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let db = h.db();
    let put: String = query_one(
        &db,
        "SELECT status FROM outbox WHERE kind = 'drawing.put'",
        &[],
    )
    .unwrap();
    assert_eq!(put, "cancelled");
    assert_eq!(puts().await, 1, "a cancelled upload is not retried");
    let delete: String = query_one(
        &db,
        "SELECT status FROM outbox WHERE kind = 'drawing.delete'",
        &[],
    )
    .unwrap();
    assert_eq!(delete, "pending", "the delete keeps retrying");

    // Registering a side again while that delete is pending does not bring
    // the unregistered drawing back from the server.
    let gets = || async {
        h.server
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .filter(|r| r.method.as_str() == "GET" && r.url.path() == "/api/v1/drawings/current")
            .count()
    };
    let gets_before = gets().await;
    h.call(&home, &side(), vec![]).await.unwrap();
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(
        query_one::<String>(&db, "SELECT sha256 FROM drawings", &[]).is_none(),
        "the deleted drawing is not re-activated"
    );
    assert_eq!(
        gets().await,
        gets_before,
        "the server copy is not fetched while its delete is pending"
    );
    drop(ui);
}

#[tokio::test]
async fn a_side_without_a_local_drawing_adopts_the_validated_server_copy() {
    use sha2::{Digest, Sha256};
    let h = Harness::start().await;
    let script = b"export default function draw(ctx) { ctx.fillRect(0, 0, 10, 10); }".to_vec();
    let sha = hex::encode(Sha256::digest(&script));
    Mock::given(method("GET"))
        .and(path("/api/v1/drawings/current"))
        .and(header("authorization", "Bearer oat_siartist"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", format!("\"{sha}\"").as_str())
                .set_body_bytes(script.clone()),
        )
        .with_priority(1)
        .expect(1)
        .mount(&h.server)
        .await;
    let ui = h.ui().await;
    let home = h.home("si:artist");
    h.call(
        &home,
        &silicon_peek_client::ipc::cli::RegisterSide {
            index: silicon_peek_client::identity::SlotIndex::new(6).unwrap(),
        },
        vec![],
    )
    .await
    .unwrap();
    let db = h.db();
    eventually(10, "the adopted drawing", || {
        query_one::<String>(&db, "SELECT sha256 FROM drawings", &[]).as_deref()
            == Some(sha.as_str())
    })
    .await;
    assert_eq!(
        ui.validated_scripts.lock().unwrap()[0],
        script,
        "validated before activation"
    );
    let sync: String = query_one(&db, "SELECT server_sync FROM drawings", &[]).unwrap();
    assert_eq!(sync, "synced", "no re-upload of the server's own copy");
    eventually(5, "drawing.load", || {
        ui.requests
            .lock()
            .unwrap()
            .iter()
            .any(|(op, _)| op == "drawing.load")
    })
    .await;
    // The Silicon can send right away.
    let mut op = send_op();
    op.show =
        Some(serde_json::from_value(json!({"elements":[{"type":"text","text":"back"}]})).unwrap());
    let (r, _) = h.call(&home, &op, vec![]).await.unwrap();
    assert_eq!(r.status, SendStatus::Showing);
}

#[tokio::test]
async fn a_failed_drawing_says_whether_a_previous_one_stays_active() {
    let h = Harness::start().await;
    let ui = h.ui().await;
    let home = h.home("si:firstdraw");
    let reg = |check_only: bool| RegisterDrawing {
        filename: "logo.js".into(),
        check_only,
        preview: false,
        dump_frame: None,
    };
    *ui.validate.lock().unwrap() = Validate::Fail("TypeError: x is null".into());
    let broken = b"export default function draw() { x.y(); }".to_vec();
    let flags = |e: &silicon_peek_client::Error| {
        let d = e.details().unwrap();
        (d["check_only"].clone(), d["previous_active"].clone())
    };
    // No drawing yet: nothing "stays active".
    let e = h
        .call(&home, &reg(false), vec![broken.clone()])
        .await
        .err()
        .unwrap();
    assert_eq!(flags(&e), (json!(false), json!(false)));
    assert!(!e.hint().unwrap().contains("previous drawing stays active"));
    let e = h
        .call(&home, &reg(true), vec![broken.clone()])
        .await
        .err()
        .unwrap();
    assert_eq!(flags(&e), (json!(true), json!(false)));
    // With a drawing registered, a failure keeps it; --check registers nothing either way.
    *ui.validate.lock().unwrap() = Validate::Ok;
    h.call(&home, &reg(false), vec![DRAWING.to_vec()])
        .await
        .unwrap();
    *ui.validate.lock().unwrap() = Validate::Fail("TypeError: x is null".into());
    let e = h
        .call(&home, &reg(false), vec![broken.clone()])
        .await
        .err()
        .unwrap();
    assert_eq!(flags(&e), (json!(false), json!(true)));
    assert!(
        e.hint()
            .unwrap()
            .contains("the previous drawing stays active")
    );
    let e = h.call(&home, &reg(true), vec![broken]).await.err().unwrap();
    assert_eq!(flags(&e), (json!(true), json!(true)));
    assert!(!e.hint().unwrap().contains("previous drawing stays active"));
}

#[tokio::test]
async fn builtin_visual_supports_immediate_and_scheduled_sends() {
    use silicon_peek_client::{identity::SlotIndex, ipc::cli::RegisterSide, timestamp::Timestamp};
    let h = Harness::start().await;
    let ui = h.ui_build(1002).await;
    let home = h.home("si:default-visual");
    h.call(
        &home,
        &RegisterSide {
            index: SlotIndex::new(3).unwrap(),
        },
        vec![],
    )
    .await
    .unwrap();
    let mut op = send_op();
    op.show = Some(
        Show::from_input(&json!({"elements":[{"type":"text","text":"Built-in visual"}]})).unwrap(),
    );
    let (sent, _) = h.call(&home, &op, vec![]).await.unwrap();
    assert_eq!(sent.status, SendStatus::Showing);
    assert_eq!(
        ui.expect("peek.show").await.fields["send_id"],
        sent.send_id.as_str()
    );
    ui.request(
        &ShownDone {
            send_id: sent.send_id,
            visible_ms: 4000,
            reason: silicon_peek_client::ipc::ui::ShownReason::Auto,
        },
        vec![],
    )
    .await
    .unwrap();
    op.due_at = Some(Timestamp::from_unix_ms(Timestamp::now().unix_ms() + 60_000));
    let (scheduled, _) = h.call(&home, &op, vec![]).await.unwrap();
    assert_eq!(scheduled.status, SendStatus::Scheduled);
    h.handle().advance_wall_clock(Duration::from_secs(61));
    assert_eq!(
        ui.expect("peek.show").await.fields["send_id"],
        scheduled.send_id.as_str()
    );
    let (status, _) = h.call(&home, &StatusOp {}, vec![]).await.unwrap();
    assert!(
        status.drawing.is_none(),
        "the built-in visual needs no drawing record"
    );
}
