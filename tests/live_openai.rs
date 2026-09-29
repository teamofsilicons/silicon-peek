//! Opt-in Gemini TTS → `OpenAI` STT round trip with fake IAM.
//! Run with `PEEK_LIVE_OPENAI=1`, `PEEK_OPENAI_API_KEY` and `PEEK_GEMINI_API_KEY`:
//! `cargo test -p silicon-peek --test live_openai -- --ignored --nocapture`.
//! Keys come only from environment variables and are never printed.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Instant;

use axum::body::Body;
use base64::Engine as _;
use common::*;
use serde_json::{Value, json};
use silicon_peek::state::Limits;

const PHRASE: &str = "The second one.";

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

fn wav(pcm: &[u8], rate: u32) -> Vec<u8> {
    let len = u32::try_from(pcm.len()).unwrap();
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(pcm);
    out
}

#[tokio::test]
#[ignore = "calls real Gemini and OpenAI APIs; needs both API keys and PEEK_LIVE_OPENAI=1"]
async fn live_proxy_speak_then_listen_round_trip() {
    let Some(key) = live_key() else { return };
    let gemini_key = std::env::var("PEEK_GEMINI_API_KEY")
        .ok()
        .filter(|key| !key.trim().is_empty())
        .expect("PEEK_GEMINI_API_KEY is required");
    let h = Harness::with(
        move |env| {
            env.insert("PEEK_OPENAI_API_KEY", key);
            env.insert("PEEK_GEMINI_API_KEY", gemini_key);
            env.insert(
                "PEEK_GEMINI_BASE_URL",
                "https://generativelanguage.googleapis.com".to_owned(),
            );
            env.insert("PEEK_OPENAI_BASE_URL", "https://api.openai.com".to_owned());
        },
        Limits::default(),
    )
    .await;
    mount_silicon(&h.iam, ACCESS).await;

    // 1. TTS chooses the Gemini proxy without minting a provider credential.
    let started = Instant::now();
    let r = h
        .send(json_body(
            authed("POST", "/api/v1/speech/token", ACCESS)
                .header("content-type", "application/json")
                .header("idempotency-key", IDEM),
            &json!({"purpose": "tts"}),
        ))
        .await;
    let token_ms = started.elapsed().as_millis();
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let token = r.json();
    eprintln!(
        "speech/token: mode {} in {token_ms} ms",
        token["mode"].as_str().unwrap_or("?")
    );
    assert_eq!(token["provider"], "gemini");

    // 2. Collect the Gemini audio for the transcription round trip.
    // tests/gemini.rs separately checks live incremental delivery.
    let response = h
        .send(json_body(
            authed("POST", "/api/v1/speech/speak", ACCESS)
                .header("content-type", "application/json")
                .header("idempotency-key", "peek-live-speak-0000000001"),
            &json!({"text": PHRASE, "model": "Kore", "sample_rate": 24000}),
        ))
        .await;
    assert_eq!(response.status, 200);
    assert_eq!(
        response.header("content-type").as_deref(),
        Some("text/event-stream")
    );
    let sse = std::str::from_utf8(&response.body).expect("UTF-8 SSE");
    let mut pcm = Vec::new();
    for data in sse.lines().filter_map(|line| line.strip_prefix("data: ")) {
        if data == "[DONE]" {
            continue;
        }
        let event: Value = serde_json::from_str(data).expect("JSON SSE");
        assert_ne!(event["event_type"], "error", "TTS stream failed");
        if event["delta"]["type"] == "audio" {
            pcm.extend(
                base64::engine::general_purpose::STANDARD
                    .decode(event["delta"]["data"].as_str().expect("audio data"))
                    .expect("base64 audio"),
            );
        }
    }
    assert!(pcm.len() > 24_000, "at least half a second of 24 kHz audio");

    // 3. STT of that audio through the proxy.
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
            .body(Body::from(wav(&pcm, 24_000)))
            .unwrap(),
        )
        .await;
    let listen_ms = started.elapsed().as_millis();
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let v: Value = r.json();
    let transcript = v["text"].as_str().unwrap_or_default().to_owned();
    eprintln!("speech/listen: {listen_ms} ms, transcript {transcript:?}");
    assert!(
        transcript.to_lowercase().contains("second"),
        "round trip transcript {transcript:?}"
    );
}
