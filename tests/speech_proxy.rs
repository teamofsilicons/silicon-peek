//! Gemini streaming, failure mapping and shared speech rate limiting.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use axum::{body::Body, http::Request};
use common::*;
use http_body_util::BodyExt as _;
use serde_json::{Value, json};
use silicon_peek::state::Limits;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tower::ServiceExt as _;
use wiremock::{
    Mock, ResponseTemplate,
    matchers::{method, path},
};

fn speak_request(body: &Value) -> Request<Body> {
    json_body(
        authed("POST", "/api/v1/speech/speak", ACCESS)
            .header("content-type", "application/json")
            .header("idempotency-key", IDEM),
        body,
    )
}

/// A raw HTTP/1.1 server standing in for Gemini: it sends the first SSE
/// chunk, then holds the stream open until the test releases it.
async fn slow_gemini(first: Vec<u8>, rest: Vec<u8>) -> (String, tokio::sync::oneshot::Sender<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (release, released) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 64 * 1024];
        let mut seen = Vec::new();
        // Read the request head and body (the JSON text is small).
        loop {
            let n = sock.read(&mut buf).await.unwrap();
            seen.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&seen);
            if let Some(end) = text.find("\r\n\r\n") {
                let len = text
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                    })
                    .unwrap_or(0);
                if seen.len() >= end + 4 + len {
                    break;
                }
            }
        }
        let chunk = |b: &[u8]| {
            let mut out = format!("{:x}\r\n", b.len()).into_bytes();
            out.extend_from_slice(b);
            out.extend_from_slice(b"\r\n");
            out
        };
        let mut head = b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n".to_vec();
        head.extend(chunk(&first));
        sock.write_all(&head).await.unwrap();
        sock.flush().await.unwrap();
        let _ = released.await;
        let mut tail = chunk(&rest);
        tail.extend_from_slice(b"0\r\n\r\n");
        sock.write_all(&tail).await.unwrap();
        sock.flush().await.unwrap();
    });
    (format!("http://{addr}"), release)
}

#[tokio::test]
async fn speak_streams_unbuffered() {
    let first = vec![1u8; 4800];
    let rest = vec![2u8; 9600];
    let (base, release) = slow_gemini(first.clone(), rest.clone()).await;
    let h = Harness::with(
        move |env| {
            env.insert("PEEK_GEMINI_BASE_URL", base);
        },
        Limits::default(),
    )
    .await;
    mount_silicon(&h.iam, ACCESS).await;
    let response = h
        .app
        .clone()
        .oneshot(speak_request(
            &json!({"text": "A long sentence.", "model": "Kore"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers()["content-type"].to_str().unwrap(),
        "text/event-stream"
    );
    let mut body = response.into_body();
    let mut got = Vec::new();
    // The first chunk arrives while Gemini is still holding the rest.
    while got.len() < first.len() {
        let frame = tokio::time::timeout(Duration::from_secs(5), body.frame())
            .await
            .expect("the first audio must arrive before the upstream finishes")
            .unwrap()
            .unwrap();
        if let Ok(data) = frame.into_data() {
            got.extend_from_slice(&data);
        }
    }
    assert_eq!(got, first);
    release.send(()).unwrap();
    let rest_got = body.collect().await.unwrap().to_bytes();
    assert_eq!(rest_got.to_vec(), rest);
}

#[tokio::test]
async fn speak_validates_and_maps_gemini_failures() {
    let h = Harness::start().await;
    mount_silicon(&h.iam, ACCESS).await;
    for bad in [
        json!({"text": "", "model": "Kore"}),
        json!({"text": "hi", "model": "bad voice"}),
        json!({"text": "hi", "model": "Kore", "sample_rate": 44100}),
        json!({"text": "hi", "model": "Kore", "voice": "x"}),
        json!({"text": "x".repeat(2001), "model": "Kore"}),
    ] {
        let r = h.send(speak_request(&bad)).await;
        assert_eq!(r.status, 400, "{bad}");
    }
    let unauthenticated = h
        .send(json_body(
            post("/api/v1/speech/speak"),
            &json!({"text": "hi", "model": "Kore"}),
        ))
        .await;
    assert_eq!(unauthenticated.status, 401);
    Mock::given(method("POST"))
        .and(path("/v1beta/interactions"))
        .respond_with(ResponseTemplate::new(401).insert_header("dg-request-id", "dg-401"))
        .mount(&h.gemini)
        .await;
    let r = h
        .send(speak_request(&json!({"text": "hi", "model": "Kore"})))
        .await;
    assert_eq!(r.status, 503);
    assert_eq!(r.code(), "speech_unavailable");
    assert_eq!(r.json()["error"]["details"]["reason"], "gemini_rejected");
    assert_eq!(r.json()["error"]["retryable"], false);
    h.gemini.reset().await;
    Mock::given(method("POST"))
        .and(path("/v1beta/interactions"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&h.gemini)
        .await;
    let r = h
        .send(speak_request(&json!({"text": "hi", "model": "Kore"})))
        .await;
    assert_eq!(r.status, 503);
    assert_eq!(r.json()["error"]["retryable"], true);
    assert!(r.header("retry-after").is_some());
}

#[tokio::test]
async fn proxied_calls_share_the_speech_rate_limit() {
    let h = Harness::with(
        |_| {},
        Limits {
            speech_per_actor_per_minute: 1,
            ..Limits::default()
        },
    )
    .await;
    mount_silicon(&h.iam, ACCESS).await;
    Mock::given(method("POST"))
        .and(path("/v1beta/interactions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(vec![0u8; 10], "text/event-stream"))
        .mount(&h.gemini)
        .await;
    let body = json!({"text": "hi", "model": "Kore"});
    assert_eq!(h.send(speak_request(&body)).await.status, 200);
    let r = h.send(speak_request(&body)).await;
    assert_eq!(r.status, 429);
    assert_eq!(r.code(), "rate_limited");
}
