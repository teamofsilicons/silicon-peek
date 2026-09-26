//! Opt-in live check of the speech proxy against the REAL Deepgram API.
//!
//! Ignored by default and skipped unless `PEEK_LIVE_DEEPGRAM=1`. The key comes
//! only from the environment at run time and is never printed:
//!
//! ```sh
//! PEEK_LIVE_DEEPGRAM=1 PEEK_DEEPGRAM_API_KEY="$(cat ~/.peek-operator/deepgram-api-key)" \
//!   cargo test -p silicon-peek --test live_deepgram -- --ignored --nocapture
//! ```
//!
//! One short TTS request and one STT request of that audio: the token route
//! (proxy or direct verdict), `POST /api/v1/speech/speak` streamed through
//! with the time to the first audio byte, then `POST /api/v1/speech/listen`
//! of the same audio as a 16 kHz WAV, which must transcribe back. IAM is a
//! local fake (wiremock); only Deepgram is real.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::{Duration, Instant};

use axum::body::Body;
use common::*;
use http_body_util::BodyExt as _;
use serde_json::{Value, json};
use silicon_peek::state::Limits;
use tower::ServiceExt as _;

const PHRASE: &str = "The second one.";

fn live_key() -> Option<String> {
    if std::env::var("PEEK_LIVE_DEEPGRAM").ok().as_deref() != Some("1") {
        eprintln!("skipped: set PEEK_LIVE_DEEPGRAM=1 (and PEEK_DEEPGRAM_API_KEY) to run");
        return None;
    }
    let key = std::env::var("PEEK_DEEPGRAM_API_KEY")
        .ok()
        .map(|k| k.trim().to_owned())
        .filter(|k| !k.is_empty());
    assert!(
        key.is_some(),
        "PEEK_LIVE_DEEPGRAM=1 needs PEEK_DEEPGRAM_API_KEY (load it from ~/.peek-operator/deepgram-api-key)"
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
#[ignore = "calls the real Deepgram API; run with PEEK_LIVE_DEEPGRAM=1"]
async fn live_proxy_speak_then_listen_round_trip() {
    let Some(key) = live_key() else { return };
    let h = Harness::with(
        move |env| {
            env.insert("PEEK_DEEPGRAM_API_KEY", key);
            env.insert(
                "PEEK_DEEPGRAM_BASE_URL",
                "https://api.deepgram.com".to_owned(),
            );
        },
        Limits::default(),
    )
    .await;
    mount_silicon(&h.iam, ACCESS).await;

    // 1. The token route decides the mode (this project's key cannot mint).
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
    assert_eq!(token["params"]["mip_opt_out"], true);

    // 2. TTS through the proxy, streamed: time to the first audio byte.
    let started = Instant::now();
    let response = h
        .app
        .clone()
        .oneshot(json_body(
            authed("POST", "/api/v1/speech/speak", ACCESS)
                .header("content-type", "application/json")
                .header("idempotency-key", "peek-live-speak-0000000001"),
            &json!({"text": PHRASE, "model": "aura-2-thalia-en", "sample_rate": 16000}),
        ))
        .await
        .unwrap();
    let head_ms = started.elapsed().as_millis();
    assert_eq!(response.status(), 200);
    let ctype = response.headers()["content-type"]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(ctype.starts_with("audio/l16"), "content-type {ctype}");
    assert!(response.headers().contains_key("dg-request-id"));
    let mut body = response.into_body();
    let mut pcm = Vec::new();
    let mut first_byte_ms = None;
    while let Some(frame) = tokio::time::timeout(Duration::from_secs(20), body.frame())
        .await
        .expect("audio kept flowing")
    {
        if let Ok(data) = frame.unwrap().into_data() {
            if first_byte_ms.is_none() && !data.is_empty() {
                first_byte_ms = Some(started.elapsed().as_millis());
            }
            pcm.extend_from_slice(&data);
        }
    }
    let total_ms = started.elapsed().as_millis();
    let first_byte_ms = first_byte_ms.expect("some audio");
    eprintln!(
        "speech/speak: headers {head_ms} ms, first audio byte {first_byte_ms} ms, done {total_ms} ms, {} ms of audio",
        pcm.len() / 32
    );
    assert!(pcm.len() > 16_000, "at least half a second of 16 kHz audio");

    // 3. STT of that audio through the proxy.
    let started = Instant::now();
    let r = h
        .send(
            authed(
                "POST",
                "/api/v1/speech/listen?model=nova-3&language=en&smart_format=true",
                ACCESS,
            )
            .header("content-type", "audio/wav")
            .header("idempotency-key", "peek-live-listen-000000001")
            .body(Body::from(wav(&pcm, 16_000)))
            .unwrap(),
        )
        .await;
    let listen_ms = started.elapsed().as_millis();
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let v: Value = r.json();
    let transcript = v["results"]["channels"][0]["alternatives"][0]["transcript"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    eprintln!("speech/listen: {listen_ms} ms, transcript {transcript:?}");
    assert!(
        transcript.to_lowercase().contains("second"),
        "round trip transcript {transcript:?}"
    );
}
