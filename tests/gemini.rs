//! Gemini TTS contract, credential isolation and streaming capacity.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use axum::{body::Body, http::Request};
use common::*;
use serde_json::{Value, json};
use silicon_peek::state::Limits;
use tower::ServiceExt as _;
use wiremock::{
    Mock, ResponseTemplate,
    matchers::{header, method, path},
};

const SSE: &str = "event: step.delta\ndata: {\"event_type\":\"step.delta\",\"delta\":{\"type\":\"audio\",\"data\":\"AAE=\"}}\n\ndata: [DONE]\n\n";

fn request(route: &str, body: &Value, test: bool) -> Request<Body> {
    let builder = authed("POST", &format!("/api/v1/speech/{route}"), ACCESS)
        .header("content-type", "application/json")
        .header("idempotency-key", IDEM);
    json_body(
        if test {
            testing(builder, Some(1))
        } else {
            builder
        },
        body,
    )
}

async fn upstream(h: &Harness, key: &str) {
    Mock::given(method("POST"))
        .and(path("/v1beta/interactions"))
        .and(header("x-goog-api-key", key))
        .respond_with(ResponseTemplate::new(200).set_body_raw(SSE, "text/event-stream"))
        .mount(&h.gemini)
        .await;
}

#[tokio::test]
async fn tts_works_without_deepgram_and_scopes_accent_metadata() {
    let h = Harness::with(
        |env| {
            env.remove("PEEK_DEEPGRAM_API_KEY");
        },
        Limits::default(),
    )
    .await;
    mount_silicon(&h.iam, ACCESS).await;
    upstream(&h, "gemini-production-key").await;
    let token = h
        .send(request("token", &json!({"purpose":"tts"}), false))
        .await;
    assert_eq!(token.status, 200);
    assert_eq!(token.json()["provider"], "gemini");
    assert_eq!(token.json()["mode"], "proxy");
    assert!(token.json().get("access_token").is_none());
    let r = h
        .send(request(
            "speak",
            &json!({"text":"Play <indian accent>Anuv Jain</indian accent>. <sigh>",
        "model":"Kore", "voice_instructions":"Warm and relaxed", "language":"en-IN"}),
            false,
        ))
        .await;
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.body, SSE.as_bytes());
    assert_eq!(r.header("x-accel-buffering").as_deref(), Some("no"));
    let sent = Harness::requests(&h.gemini, "/v1beta/interactions").await;
    let payload: Value = serde_json::from_slice(&sent[0].body).unwrap();
    assert_eq!(payload["model"], "gemini-3.8-flash-tts");
    assert_eq!(payload["stream"], true);
    assert_eq!(payload["store"], false);
    assert_eq!(
        payload["response_format"],
        json!({"type":"audio", "mime_type":"audio/l16", "sample_rate":24000})
    );
    assert_eq!(
        payload["generation_config"]["speech_config"],
        json!([{"voice":"Kore"}])
    );
    let blocks = &payload["input"][0]["content"];
    assert_eq!(blocks[1]["text"], "Anuv Jain");
    assert_eq!(
        blocks[1]["annotations"][0]["style"],
        "Warm and relaxed\nSpeak in language en-IN.\nUse an indian accent for this text."
    );
    assert_eq!(blocks[2]["text"], ". <sigh>");
    assert!(
        Harness::requests(&h.deepgram, "/v1/auth/grant")
            .await
            .is_empty()
    );
    let events = serde_json::to_string(&h.events()).unwrap();
    for private in ["gemini-production-key", "Anuv Jain", "Warm and relaxed"] {
        assert!(!events.contains(private));
    }
}

#[tokio::test]
async fn testing_key_never_falls_back_to_production() {
    for configured in [false, true] {
        let h = Harness::with(
            |env| {
                if !configured {
                    env.remove("PEEK_GEMINI_TEST_API_KEY");
                }
            },
            Limits::default(),
        )
        .await;
        mount_testing_contexts(&h.iam).await;
        h.prepare_environment().await;
        mount_introspect_testing(
            &h.iam,
            ACCESS,
            introspection(ACTOR, ORG, &FULL_SCOPES, Some("admin"), Some(env_id())),
        )
        .await;
        upstream(&h, "gemini-testing-key").await;
        for (route, body) in [
            ("token", json!({"purpose":"tts"})),
            ("speak", json!({"text":"Hi", "model":"Kore"})),
        ] {
            let r = h.send(request(route, &body, true)).await;
            assert_eq!(
                r.status.as_u16(),
                if configured { 200 } else { 503 },
                "{}",
                String::from_utf8_lossy(&r.body)
            );
            if !configured {
                assert_eq!(r.json()["error"]["details"]["reason"], "not_configured");
            }
        }
        assert_eq!(
            Harness::requests(&h.gemini, "/v1beta/interactions")
                .await
                .len(),
            usize::from(configured)
        );
    }
}

#[tokio::test]
async fn more_than_twenty_five_streams_can_remain_open() {
    let h = Harness::start().await;
    mount_silicon(&h.iam, ACCESS).await;
    upstream(&h, "gemini-production-key").await;
    let mut responses = Vec::new();
    // Keep every response unconsumed so generation streams remain owned by the callers.
    for _ in 0..26 {
        let response = h
            .app
            .clone()
            .oneshot(request(
                "speak",
                &json!({"text":"Hi", "model":"Kore"}),
                false,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        responses.push(response);
    }
    assert_eq!(
        Harness::requests(&h.gemini, "/v1beta/interactions")
            .await
            .len(),
        responses.len()
    );
}

#[tokio::test]
async fn errors_do_not_echo_provider_bodies_and_validation_stays_local() {
    let h = Harness::start().await;
    mount_silicon(&h.iam, ACCESS).await;
    for body in [
        json!({"text":"Hi", "model":"Kore", "voice_instructions":"x".repeat(2001)}),
        json!({"text":"Hi", "model":"Kore", "language":"en/../../"}),
        json!({"text":"<indian accent>Hi", "model":"Kore"}),
    ] {
        assert_eq!(h.send(request("speak", &body, false)).await.status, 400);
    }
    assert!(
        Harness::requests(&h.gemini, "/v1beta/interactions")
            .await
            .is_empty()
    );
    for (status, expected) in [(400, 400), (401, 503), (403, 503), (429, 503), (500, 503)] {
        h.gemini.reset().await;
        Mock::given(method("POST"))
            .and(path("/v1beta/interactions"))
            .respond_with(
                ResponseTemplate::new(status)
                    .insert_header("retry-after", "9")
                    .set_body_string("private transcript and key"),
            )
            .mount(&h.gemini)
            .await;
        let r = h
            .send(request(
                "speak",
                &json!({"text":"Hi", "model":"Kore"}),
                false,
            ))
            .await;
        assert_eq!(r.status.as_u16(), expected);
        assert!(!String::from_utf8_lossy(&r.body).contains("private transcript"));
        if status == 429 {
            assert_eq!(r.header("retry-after").as_deref(), Some("9"));
        }
    }
}

#[tokio::test]
#[ignore = "calls real Gemini TTS; requires PEEK_GEMINI_API_KEY"]
async fn live_gemini_stream() {
    use base64::Engine as _;
    use http_body_util::BodyExt as _;
    use std::time::{Duration, Instant};

    let key = std::env::var("PEEK_GEMINI_API_KEY").expect("PEEK_GEMINI_API_KEY is required");
    assert!(!key.trim().is_empty(), "PEEK_GEMINI_API_KEY is empty");
    let h = Harness::with(
        move |env| {
            env.insert("PEEK_GEMINI_API_KEY", key);
            env.insert(
                "PEEK_GEMINI_BASE_URL",
                "https://generativelanguage.googleapis.com".to_owned(),
            );
            env.remove("PEEK_DEEPGRAM_API_KEY");
        },
        Limits::default(),
    )
    .await;
    mount_silicon(&h.iam, ACCESS).await;
    let started = Instant::now();
    let response = h.app.clone().oneshot(request("speak", &json!({
        "text": "Playing <indian accent>Anuv Jain</indian accent>. <short pause> Have a wonderful day!",
        "model": "Kore", "voice_instructions": "Warm and relaxed"
    }), false)).await.unwrap();
    assert_eq!(response.status(), 200);
    let mut body = response.into_body();
    let mut pending = Vec::new();
    let mut audio_chunks = 0;
    let mut audio_bytes = 0;
    let mut first_audio_ms = None;
    let mut complete = false;
    while let Some(frame) = tokio::time::timeout(Duration::from_secs(30), body.frame())
        .await
        .expect("stream stalled")
    {
        if let Ok(bytes) = frame.unwrap().into_data() {
            pending.extend_from_slice(&bytes);
            while let Some(end) = pending.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = pending.drain(..=end).collect();
                let line = std::str::from_utf8(&line).unwrap().trim();
                let Some(data) = line.strip_prefix("data:").map(str::trim) else {
                    continue;
                };
                if data == "[DONE]" {
                    continue;
                }
                let event: Value = serde_json::from_str(data).unwrap();
                assert_ne!(event["event_type"], "error", "Gemini stream failed");
                if event["delta"]["type"] == "audio" {
                    first_audio_ms.get_or_insert_with(|| started.elapsed().as_millis());
                    audio_chunks += 1;
                    audio_bytes += base64::engine::general_purpose::STANDARD
                        .decode(event["delta"]["data"].as_str().unwrap())
                        .unwrap()
                        .len();
                }
                complete |= event["event_type"] == "interaction.completed";
            }
        }
    }
    assert!(
        complete && audio_chunks > 1 && audio_bytes > 24_000,
        "stream must complete with multiple PCM chunks"
    );
    eprintln!(
        "Gemini relay: first audio {} ms, {audio_chunks} audio chunks, {audio_bytes} PCM bytes, total {} ms",
        first_audio_ms.unwrap(),
        started.elapsed().as_millis()
    );
}
