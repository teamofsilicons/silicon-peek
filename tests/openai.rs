//! `OpenAI` STT uploads, isolation and bounded failures against a local provider.
#![allow(clippy::unwrap_used, clippy::expect_used)]
mod common;
use axum::{body::Body, http::Request};
use common::*;
use serde_json::json;
use silicon_peek::state::Limits;
use wiremock::{
    Mock, ResponseTemplate,
    matchers::{header, method, path},
};

fn listen(query: &str, audio: Vec<u8>, test: bool) -> Request<Body> {
    let b = authed("POST", &format!("/api/v1/speech/listen?{query}"), ACCESS)
        .header("content-type", "audio/wav")
        .header("idempotency-key", IDEM);
    (if test { testing(b, Some(1)) } else { b })
        .body(Body::from(audio))
        .unwrap()
}
fn token(test: bool) -> Request<Body> {
    let b = authed("POST", "/api/v1/speech/token", ACCESS)
        .header("content-type", "application/json")
        .header("idempotency-key", IDEM);
    json_body(
        if test { testing(b, Some(1)) } else { b },
        &json!({"purpose":"stt"}),
    )
}
async fn upstream(h: &Harness, key: &str) {
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .and(header("authorization", format!("Bearer {key}").as_str()))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-request-id", "req-openai")
                .set_body_json(json!({"text":"The second one.","languages":[{"code":"en"}]})),
        )
        .mount(&h.openai)
        .await;
}

#[tokio::test]
async fn transcribes_without_deepgram_and_translates_query_into_multipart() {
    let h = Harness::with(
        |env| {
            env.remove("PEEK_DEEPGRAM_API_KEY");
        },
        Limits::default(),
    )
    .await;
    mount_silicon(&h.iam, ACCESS).await;
    upstream(&h, "openai-production-key").await;
    let t = h.send(token(false)).await;
    assert_eq!(t.status, 200);
    assert_eq!(t.json()["provider"], "openai");
    assert_eq!(t.json()["mode"], "proxy");
    assert!(t.json().get("access_token").is_none());
    let audio = vec![5u8; 2 * 1024 * 1024];
    let r=h.send(listen("model=gpt-transcribe&language=en-US&detect_language=en&detect_language=hi&keyterm=The+second+one&numerals=true&smart_format=true",audio.clone(),false)).await;
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(
        r.json(),
        json!({"text":"The second one.","request_id":"req-openai","detected_language":"en"})
    );
    let sent = Harness::requests(&h.openai, "/v1/audio/transcriptions").await;
    let raw = String::from_utf8_lossy(&sent[0].body);
    assert!(raw.contains("filename=\"audio.wav\""));
    assert!(raw.contains("Content-Type: audio/wav") || raw.contains("content-type: audio/wav"));
    for (name, value) in [
        ("model", "gpt-transcribe"),
        ("response_format", "json"),
        ("languages[]", "en"),
        ("languages[]", "hi"),
        ("keywords[]", "The second one"),
        ("prompt", "Write spoken numbers as digits."),
    ] {
        assert!(
            raw.contains(&format!("name=\"{name}\"\r\n\r\n{value}\r\n")),
            "missing {name}"
        );
    }
    assert_eq!(raw.matches("name=\"languages[]\"").count(), 2);
    assert!(!raw.contains("name=\"language\""));
    assert!(!raw.contains("smart_format"));
    assert!(sent[0].body.windows(audio.len()).any(|part| part == audio));
    h.state.maintenance().await;
    assert!(h.deepgram.received_requests().await.unwrap().is_empty());
    let events = serde_json::to_string(&h.events()).unwrap();
    assert!(events.contains("stt_request_id"));
    for private in ["The second one", "openai-production-key"] {
        assert!(!events.contains(private));
    }
}

#[tokio::test]
async fn testing_key_never_falls_back_to_production_or_deepgram() {
    for configured in [false, true] {
        let h = Harness::with(
            |env| {
                if !configured {
                    env.remove("PEEK_OPENAI_TEST_API_KEY");
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
        upstream(&h, "openai-testing-key").await;
        for req in [token(true), listen("", vec![1; 10], true)] {
            let r = h.send(req).await;
            assert_eq!(r.status.as_u16(), if configured { 200 } else { 503 });
            if !configured {
                assert_eq!(r.json()["error"]["details"]["reason"], "not_configured");
            }
        }
        assert_eq!(
            Harness::requests(&h.openai, "/v1/audio/transcriptions")
                .await
                .len(),
            usize::from(configured)
        );
        assert!(h.deepgram.received_requests().await.unwrap().is_empty());
    }
}

#[tokio::test]
async fn missing_production_key_does_not_use_testing_or_legacy_keys() {
    let h = Harness::with(
        |env| {
            env.remove("PEEK_OPENAI_API_KEY");
        },
        Limits::default(),
    )
    .await;
    mount_silicon(&h.iam, ACCESS).await;
    for req in [token(false), listen("", vec![1], false)] {
        let r = h.send(req).await;
        assert_eq!(r.status, 503);
        assert_eq!(r.json()["error"]["details"]["reason"], "not_configured");
    }
    assert!(h.openai.received_requests().await.unwrap().is_empty());
    assert!(h.deepgram.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn transport_failure_is_retryable() {
    let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap();
    drop(socket);
    let h = Harness::with(
        |env| {
            env.insert("PEEK_OPENAI_BASE_URL", format!("http://{address}"));
        },
        Limits::default(),
    )
    .await;
    mount_silicon(&h.iam, ACCESS).await;
    let r = h.send(listen("", vec![1], false)).await;
    assert_eq!(r.status, 503);
    assert_eq!(r.json()["error"]["retryable"], true);
    assert_eq!(r.json()["error"]["details"]["reason"], "openai_unavailable");
}

#[tokio::test]
async fn validates_before_upload_and_maps_provider_failures_without_echo() {
    let h = Harness::start().await;
    mount_silicon(&h.iam, ACCESS).await;
    for query in [
        "model=nova-3",
        "keyterm=%3Cbad%3E",
        "keyterm=bad%0Aline",
        "keyterm=",
        "language=en%20US",
        "callback=https://x",
        "numerals=yes",
    ] {
        assert_eq!(
            h.send(listen(query, vec![1], false)).await.status,
            400,
            "{query}"
        );
    }
    assert_eq!(h.send(listen("", vec![], false)).await.status, 400);
    assert_eq!(
        h.send(listen("", vec![1; 4 * 1024 * 1024 + 1], false))
            .await
            .status,
        413
    );
    let mut req = listen("", vec![1], false);
    req.headers_mut()
        .insert("content-type", "audio/mpeg".parse().unwrap());
    assert_eq!(h.send(req).await.status, 415);
    let mut req = listen("", vec![1], false);
    req.headers_mut().remove("authorization");
    assert_eq!(h.send(req).await.status, 401);
    assert!(h.openai.received_requests().await.unwrap().is_empty());
    for (status, expected, retry) in [
        (400, 400, false),
        (401, 503, false),
        (403, 503, false),
        (408, 503, true),
        (429, 503, true),
        (500, 503, true),
    ] {
        h.openai.reset().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(
                ResponseTemplate::new(status)
                    .insert_header("retry-after", "8")
                    .set_body_string("private provider key and transcript"),
            )
            .mount(&h.openai)
            .await;
        let r = h.send(listen("", vec![1], false)).await;
        assert_eq!(r.status.as_u16(), expected);
        assert_eq!(r.json()["error"]["retryable"], retry);
        assert!(!String::from_utf8_lossy(&r.body).contains("private provider"));
        if retry {
            assert_eq!(r.header("retry-after").as_deref(), Some("8"));
        }
    }
}

#[tokio::test]
async fn malformed_or_oversized_results_fail_and_empty_language_list_is_valid() {
    let h = Harness::start().await;
    mount_silicon(&h.iam, ACCESS).await;
    for template in [
        ResponseTemplate::new(200).set_body_raw("not json", "application/json"),
        ResponseTemplate::new(200).set_body_json(json!({"languages":[]})),
        ResponseTemplate::new(200).set_body_json(json!({"text":null})),
        ResponseTemplate::new(200).set_body_raw("x".repeat(1024 * 1024 + 1), "application/json"),
        ResponseTemplate::new(200).set_body_raw("{}", "text/plain"),
    ] {
        h.openai.reset().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(template)
            .mount(&h.openai)
            .await;
        let r = h.send(listen("", vec![1], false)).await;
        assert_eq!(r.status, 503);
        assert_eq!(r.json()["error"]["details"]["reason"], "openai_unavailable");
    }
    h.openai.reset().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"text":"","languages":[]})))
        .mount(&h.openai)
        .await;
    assert_eq!(
        h.send(listen("", vec![1], false)).await.json(),
        json!({"text":""})
    );
}
