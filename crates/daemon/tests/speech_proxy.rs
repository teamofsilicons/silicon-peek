//! Speech proxy mode from peekd's side: when peek-server's token reply says
//! `{"mode":"proxy"}`, TTS and STT go through `/api/v1/speech/speak|listen`
//! with the Silicon's own session (never to another host), a refused session
//! is refreshed once, and the TTS stream reaches the UI exactly as in direct
//! mode (`tts.begin` with `est_frames`, `tts.chunk` from seq 0, `tts.end`
//! with `total_frames`). Plus the IPC additions of the integration pass.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::too_many_lines)]

mod common;

use std::time::Duration;

use common::{Harness, eventually, pcm};
use serde_json::{Value, json};
use silicon_peek_client::{
    ErrorCode,
    identity::{Context, SlotIndex},
    ipc::{
        cli::{AppUninstall, Doctor, RegisterSide, SendOp},
        ui::{MessageOp, SettingsChanged, UiStatusReport, VoiceSubmit},
    },
    schema::ask::Ask,
    ting::MessageVia,
};
use silicon_peek_daemon::{
    stt::{tone, wav_bytes},
    voice::estimated_frames,
};
use wiremock::{
    Mock, ResponseTemplate,
    matchers::{header, header_exists, method, path, query_param},
};

fn send_op() -> SendOp {
    SendOp {
        isi: Some("deliberate".into()),
        speak: None,
        show: None,
        ask: None,
        voice: None,
        lang: None,
        notify: vec![],
        duration_ms: None,
        expires_in_s: None,
        wait: false,
    }
}

/// Overrides the default direct token with a proxy verdict for `base`.
async fn proxy_token(h: &Harness, base: &str) {
    Mock::given(method("POST"))
        .and(path("/api/v1/speech/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "mode": "proxy", "expires_in": 600, "base_url": base,
            "key_source": "peek", "params": {"mip_opt_out": true, "tags": ["peek", "development"]}
        })))
        .with_priority(1)
        .mount(&h.server)
        .await;
}

#[tokio::test]
async fn proxy_mode_streams_tts_through_the_backend_with_the_silicons_session() {
    let h = Harness::start().await;
    proxy_token(&h, &format!("{}/api/v1/speech", h.server.uri())).await;
    let audio = pcm(90_001);
    Mock::given(method("POST"))
        .and(path("/api/v1/speech/speak"))
        .and(header("authorization", "Bearer oat_sicleanup"))
        .and(header("x-org-id", "tos"))
        .and(header_exists("idempotency-key"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "audio/l16;rate=24000;channels=1")
                .insert_header("dg-request-id", "dg-proxy-1")
                .set_body_bytes(audio.clone()),
        )
        .expect(1)
        .mount(&h.server)
        .await;
    let ui = h.ui().await;
    let home = h.home("si:cleanup");
    h.register(&home, 3, &ui).await;
    let text = "The build finished. Deploying to staging now.";
    let mut op = send_op();
    op.speak = Some(text.into());
    let (r, _) = h.call(&home, &op, vec![]).await.unwrap();
    let _ = ui.expect("peek.show").await;
    let (begin, bytes, total) = ui.tts_stream(r.send_id.as_str()).await;
    assert_eq!(begin["format"], "s16le");
    assert_eq!(begin["sample_rate"], 24000);
    assert_eq!(
        begin["est_frames"],
        estimated_frames(text.chars().count()),
        "est_frames ≈ chars ÷ 14 × 24000"
    );
    assert_eq!(bytes, audio[..90_000], "every whole sample, in order");
    assert_eq!(total, 45_000, "total_frames = bytes ÷ 2");
    let sent = h
        .server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.url.path() == "/api/v1/speech/speak")
        .unwrap();
    let body: Value = serde_json::from_slice(&sent.body).unwrap();
    assert_eq!(
        body,
        json!({"text": text, "model": "aura-2-thalia-en", "sample_rate": 24000})
    );
    assert!(
        h.deepgram.received_requests().await.unwrap().is_empty(),
        "nothing goes to Deepgram directly in proxy mode"
    );
}

#[tokio::test]
async fn proxy_mode_transcribes_through_the_backend() {
    let h = Harness::start().await;
    h.accept_deliveries().await;
    proxy_token(&h, &format!("{}/api/v1/speech", h.server.uri())).await;
    Mock::given(method("POST"))
        .and(path("/api/v1/speech/listen"))
        .and(header("authorization", "Bearer oat_sicleanup"))
        .and(header("content-type", "audio/wav"))
        .and(query_param("model", "nova-3"))
        .and(query_param("smart_format", "true"))
        .and(query_param("keyterm", "Keep it"))
        .and(query_param("language", "en"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("dg-request-id", "dg-listen-1")
                .set_body_json(json!({
                    "results": {"channels": [{"alternatives": [{"transcript": "the second one"}]}]}
                })),
        )
        .expect(1)
        .mount(&h.server)
        .await;
    let ui = h.ui().await;
    let home = h.home("si:cleanup");
    h.register(&home, 7, &ui).await;
    let mut op = send_op();
    op.ask = Some(
        Ask::from_input(
            &json!({"question":"Delete ~/Downloads/old.zip?","type":"single_choice",
            "options":[{"id":"keep","label":"Keep it"},{"id":"delete","label":"Delete"}]}),
        )
        .unwrap(),
    );
    let (r, _) = h.call(&home, &op, vec![]).await.unwrap();
    let _ = ui.expect("peek.show").await;
    let wav = wav_bytes(&tone(1500, 8000), 16_000);
    ui.request(
        &VoiceSubmit {
            send_id: Some(r.send_id.clone()),
            ask_id: r.ask_id.clone(),
            slot: r.slot,
            duration_ms: 1500,
            languages: vec!["en-IN".into()],
            context: Some(Context::Production),
        },
        vec![wav.clone()],
    )
    .await
    .unwrap();
    let res = ui.expect("stt.result").await;
    assert_eq!(res.fields["outcome"], "matched");
    assert_eq!(res.fields["value"], "delete");
    let sent = h
        .server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.url.path() == "/api/v1/speech/listen")
        .unwrap();
    assert_eq!(sent.body, wav);
    assert!(
        !sent
            .url
            .query_pairs()
            .any(|(k, _)| k == "tag" || k == "mip_opt_out"),
        "the backend adds tags and mip_opt_out itself"
    );
}

#[tokio::test]
async fn a_refused_session_is_refreshed_once_and_a_foreign_proxy_is_never_used() {
    let h = Harness::start().await;
    proxy_token(&h, &format!("{}/api/v1/speech", h.server.uri())).await;
    // The first speak is refused (the access token was revoked); peekd
    // refreshes the session and retries once with the new token.
    Mock::given(method("POST"))
        .and(path("/api/v1/speech/speak"))
        .and(header("authorization", "Bearer oat_sicleanup"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": {"code": "unauthenticated", "message": "inactive", "retryable": false}
        })))
        .mount(&h.server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/refresh"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "oat_refreshed01", "refresh_token": "ort_refreshed01", "token_type": "Bearer",
            "expires_in": 1800, "scope": common::FULL_SCOPE,
            "actor": {"type": "silicon", "public_id": "si:cleanup"}, "org_id": "tos", "org_ids": ["tos"],
            "membership_id": "si:cleanup[tos]"
        })))
        .expect(1)
        .mount(&h.server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/speech/speak"))
        .and(header("authorization", "Bearer oat_refreshed01"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "audio/l16;rate=24000")
                .set_body_bytes(pcm(4800)),
        )
        .expect(1)
        .mount(&h.server)
        .await;
    let ui = h.ui().await;
    let home = h.home("si:cleanup");
    h.register(&home, 1, &ui).await;
    let mut op = send_op();
    op.speak = Some("hello".into());
    let (r, _) = h.call(&home, &op, vec![]).await.unwrap();
    let (_, bytes, _) = ui.tts_stream(r.send_id.as_str()).await;
    assert_eq!(bytes.len(), 4800);

    // A proxy URL that is not this home's backend is refused: the session
    // is never sent anywhere else and the UI shows the text instead.
    let other = Harness::start().await;
    proxy_token(&other, "http://127.0.0.1:9/api/v1/speech").await;
    let ui2 = other.ui().await;
    let home2 = other.home("si:cleanup");
    other.register(&home2, 2, &ui2).await;
    let (r2, _) = other.call(&home2, &op, vec![]).await.unwrap();
    let err = ui2.expect("tts.error").await;
    assert_eq!(err.fields["send_id"], r2.send_id.as_str());
    assert_eq!(err.fields["error"]["code"], "speech_unavailable");
    assert!(
        err.fields["error"]["message"]
            .as_str()
            .unwrap()
            .contains("never sends a session elsewhere")
    );
}

#[tokio::test]
async fn direct_mode_always_opts_out_of_mip() {
    let h = Harness::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/speech/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "jwt-nomip", "expires_in": 60, "base_url": h.deepgram.uri(),
            "key_source": "peek", "params": {"mip_opt_out": false, "tags": ["peek", "production"]}
        })))
        .with_priority(1)
        .mount(&h.server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/speak"))
        .and(query_param("mip_opt_out", "true"))
        .and(header("authorization", "Bearer jwt-nomip"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "audio/l16;rate=24000")
                .set_body_bytes(pcm(480)),
        )
        .expect(1)
        .mount(&h.deepgram)
        .await;
    let ui = h.ui().await;
    let home = h.home("si:cleanup");
    h.register(&home, 4, &ui).await;
    let mut op = send_op();
    op.speak = Some("hello again".into());
    let (r, _) = h.call(&home, &op, vec![]).await.unwrap();
    let (_, bytes, _) = ui.tts_stream(r.send_id.as_str()).await;
    assert_eq!(bytes.len(), 480);
}

#[tokio::test]
async fn doctor_and_uninstall_are_relayed_to_the_ui() {
    let h = Harness::start().await;
    // No UI yet: doctor still answers (UI part null, with the reason);
    // uninstall fails so the CLI can fall back to `open --args --uninstall`.
    let mut cli = h.cli().await;
    let (report, _) = cli
        .call(&Doctor {}, None, vec![], Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(report["ui_running"], false);
    assert!(report["mic"].is_null());
    assert!(report["ui_detail"].as_str().is_some());
    assert_eq!(report["peekd"]["version"], silicon_peek_client::VERSION);
    let mut cli = h.cli().await;
    let e = cli
        .call(&AppUninstall {}, None, vec![], Duration::from_secs(5))
        .await
        .err()
        .unwrap();
    assert_eq!(*e.code(), ErrorCode::PeekServiceUnavailable);

    let ui = h.ui().await;
    let home = h.home("si:cleanup");
    let mut cli = h.cli().await;
    let (report, _) = cli
        .call(&Doctor {}, Some(&home.auth), vec![], Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(report["ui_running"], true);
    assert_eq!(report["mic"], "granted");
    assert_eq!(report["hotkeys"]["registered"], json!(["ctrl+cmd+3"]));
    assert_eq!(report["ui_status_source"], "live");

    // The app pushes `ui.status`; when the live relay cannot be answered,
    // doctor shows what the app last pushed.
    ui.request(
        &serde_json::from_value::<UiStatusReport>(json!({
            "mic": "denied",
            "hotkeys": {"modifier": "ctrl+cmd", "registered": ["ctrl+cmd+5"], "failed": ["ctrl+cmd+2"],
                        "problems": ["ctrl+cmd+2 is already taken by another app"]},
            "glass": "live", "services": "enabled", "app_build": 1000, "app_version": "0.1.0"
        }))
        .unwrap(),
        vec![],
    )
    .await
    .unwrap();
    *ui.answers_doctor.lock().unwrap() = false;
    let mut cli = h.cli().await;
    let (report, _) = cli
        .call(&Doctor {}, None, vec![], Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(report["mic"], "denied");
    assert_eq!(report["hotkeys"]["failed"], json!(["ctrl+cmd+2"]));
    assert_eq!(report["app_build"], 1000);
    assert_eq!(report["ui_status_source"], "pushed");
    *ui.answers_doctor.lock().unwrap() = true;
    let mut cli = h.cli().await;
    let (r, _) = cli
        .call(&AppUninstall {}, None, vec![], Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(r["accepted"], true);
    assert_eq!(r["via"], "ui");
    assert!(
        ui.requests
            .lock()
            .unwrap()
            .iter()
            .any(|(op, _)| op == "app.uninstall")
    );
}

#[tokio::test]
async fn drawings_are_staged_under_their_original_file_name() {
    let h = Harness::start().await;
    let ui = h.ui().await;
    let home = h.home("si:cleanup");
    h.register(&home, 5, &ui).await;
    let validate = ui
        .requests
        .lock()
        .unwrap()
        .iter()
        .find(|(op, _)| op == "drawing.validate")
        .map(|(_, f)| f.clone())
        .unwrap();
    let script = validate["script_path"].as_str().unwrap().to_owned();
    assert!(script.ends_with("/logo.js"), "{script}");
    assert!(script.contains("/.validate-"), "{script}");
    let staging = std::path::Path::new(&script)
        .parent()
        .unwrap()
        .to_path_buf();
    eventually(5, "the staging directory to be removed", || {
        !staging.exists()
    })
    .await;
}

#[tokio::test]
async fn ui_context_addresses_the_registration_and_settings_merge() {
    let h = Harness::start().await;
    h.accept_deliveries().await;
    let ui = h.ui().await;
    let home = h.home("si:cleanup");
    h.register(&home, 6, &ui).await;
    // A message for the testing partition of slot 6 finds nobody there.
    let e = ui
        .request(
            &MessageOp {
                slot: SlotIndex::new(6).unwrap(),
                text: "hello".into(),
                via: MessageVia::Keyboard,
                context: Some(Context::parse("0192f2d2-7c9e-7cc0-8b2e-6f3a2b1c0d9e").unwrap()),
            },
            vec![],
        )
        .await
        .err()
        .unwrap();
    assert_eq!(*e.code(), ErrorCode::SideNotRegistered);
    let ok = ui
        .request(
            &MessageOp {
                slot: SlotIndex::new(6).unwrap(),
                text: "hello".into(),
                via: MessageVia::Keyboard,
                context: Some(Context::Production),
            },
            vec![],
        )
        .await
        .unwrap();
    assert!(ok.message_id.as_str().starts_with("cmsg_"));

    // Peek.app wrote settings.json (with a key peekd does not know), then
    // sends settings.changed; peekd merges and keeps the UI's keys.
    let settings = h.cfg.support_dir.join("settings.json");
    std::fs::write(
        &settings,
        br#"{"schema":1,"hotkey_modifier":"opt+cmd","mode":"compact","ui_only":true}"#,
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&settings, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    ui.request(
        &SettingsChanged {
            key: "hotkey_modifier".into(),
            value: json!("opt+cmd"),
        },
        vec![],
    )
    .await
    .unwrap();
    let on_disk: Value = serde_json::from_slice(&std::fs::read(&settings).unwrap()).unwrap();
    assert_eq!(on_disk["ui_only"], true);
    assert_eq!(on_disk["mode"], "compact");
    assert_eq!(on_disk["schema"], 1);
    let (r, _) = h
        .call(
            &home,
            &RegisterSide {
                index: SlotIndex::new(6).unwrap(),
            },
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(r.hotkey.as_deref(), Some("opt+cmd+6"));
}
