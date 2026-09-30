//! Opt-in `OpenAI` transcription with fake IAM and a caller-provided WAV recording.
//! Run with `PEEK_LIVE_OPENAI=1`, `PEEK_OPENAI_API_KEY` and `PEEK_LIVE_AUDIO_FILE`:
//! `cargo test -p silicon-peek --test live_openai -- --ignored --nocapture`.
//! Keys come only from environment variables and are never printed.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Instant;

use axum::body::Body;
use common::*;
use serde_json::Value;
use silicon_peek::state::Limits;

fn live_key() -> Option<String> {
    if std::env::var("PEEK_LIVE_OPENAI").ok().as_deref() != Some("1") {
        eprintln!("skipped: set PEEK_LIVE_OPENAI=1 (and PEEK_OPENAI_API_KEY) to run");
        return None;
    }
    let key = std::env::var("PEEK_OPENAI_API_KEY")
        .ok()
        .map(|k| k.trim().to_owned())
        .filter(|k| !k.is_empty());
    assert!(
        key.is_some(),
        "PEEK_LIVE_OPENAI=1 needs PEEK_OPENAI_API_KEY"
    );
    key
}

#[tokio::test]
#[ignore = "calls real OpenAI; needs PEEK_OPENAI_API_KEY, PEEK_LIVE_AUDIO_FILE and PEEK_LIVE_OPENAI=1"]
async fn live_transcription() {
    let Some(key) = live_key() else { return };
    let wav = std::fs::read(
        std::env::var("PEEK_LIVE_AUDIO_FILE").expect("PEEK_LIVE_AUDIO_FILE is required"),
    )
    .expect("read recorded WAV");
    let h = Harness::with(
        move |env| {
            env.insert("PEEK_OPENAI_API_KEY", key);
            env.insert("PEEK_OPENAI_BASE_URL", "https://api.openai.com".to_owned());
        },
        Limits::default(),
    )
    .await;
    mount_silicon(&h.iam, ACCESS).await;

    // Send a completed recorded WAV through the real OpenAI adapter.
    let started = Instant::now();
    let r = h
        .send(
            authed(
                "POST",
                "/api/v1/speech/listen?model=gpt-transcribe&language=en&smart_format=true",
                ACCESS,
            )
            .header("content-type", "audio/wav")
            .header("idempotency-key", "peek-live-listen-000000001")
            .body(Body::from(wav))
            .unwrap(),
        )
        .await;
    let listen_ms = started.elapsed().as_millis();
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let v: Value = r.json();
    eprintln!("speech/listen: {listen_ms} ms");
    assert!(v["text"].is_string());
}
