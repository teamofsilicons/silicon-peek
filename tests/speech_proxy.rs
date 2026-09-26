//! Speech proxy mode: the token route's `{"mode":"proxy"}` verdict (cached
//! per key), `POST /api/v1/speech/speak` streaming Deepgram's audio through
//! unbuffered, and `POST /api/v1/speech/listen` with allow-listed
//! parameters. Deepgram is a local fake; nothing talks to a real service.

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
    matchers::{body_json, header, method, path, query_param},
};

fn token_request() -> Request<Body> {
    json_body(
        authed("POST", "/api/v1/speech/token", ACCESS)
            .header("content-type", "application/json")
            .header("idempotency-key", IDEM),
        &json!({"purpose": "tts"}),
    )
}

fn speak_request(body: &Value) -> Request<Body> {
    json_body(
        authed("POST", "/api/v1/speech/speak", ACCESS)
            .header("content-type", "application/json")
            .header("idempotency-key", IDEM),
        body,
    )
}

fn listen_request(query: &str, audio: Vec<u8>) -> Request<Body> {
    authed("POST", &format!("/api/v1/speech/listen?{query}"), ACCESS)
        .header("content-type", "audio/wav")
        .header("idempotency-key", IDEM)
        .body(Body::from(audio))
        .unwrap()
}

async fn forbid_grant(h: &Harness) {
    Mock::given(method("POST"))
        .and(path("/v1/auth/grant"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "category": "FORBIDDEN", "message": "Insufficient permissions."
        })))
        .expect(1)
        .mount(&h.deepgram)
        .await;
}

#[tokio::test]
async fn a_key_that_cannot_mint_answers_proxy_mode_and_the_verdict_is_cached() {
    let h = Harness::start().await;
    mount_silicon(&h.iam, ACCESS).await;
    forbid_grant(&h).await;
    for _ in 0..3 {
        let r = h.send(token_request()).await;
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
        let v = r.json();
        assert_eq!(v["mode"], "proxy");
        assert!(
            v.get("access_token").is_none(),
            "no credential in proxy mode"
        );
        assert_eq!(
            v["base_url"],
            "https://backend.peek.teamofsilicons.com/api/v1/speech"
        );
        assert_eq!(v["key_source"], "peek");
        assert_eq!(
            v["params"],
            json!({"mip_opt_out": true, "tags": ["peek", "development"]})
        );
        assert!(v["expires_in"].as_u64().is_some_and(|e| e > 0 && e <= 600));
    }
    // `expect(1)`: the forbidden verdict is cached, /v1/auth/grant is asked once.
    h.deepgram.verify().await;
    let event = h
        .events()
        .into_iter()
        .find(|e| e["event"] == "speech.token")
        .unwrap();
    assert_eq!(event["context"]["method"], "proxy");
}

#[tokio::test]
async fn the_proxy_verdict_is_prewarmed_so_the_first_token_skips_the_grant_probe() {
    let h = Harness::start().await;
    mount_silicon(&h.iam, ACCESS).await;
    Mock::given(method("POST"))
        .and(path("/v1/auth/grant"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "category": "FORBIDDEN", "message": "Insufficient permissions."
        })))
        // Start-up probes the production and the testing key once each.
        .expect(2)
        .mount(&h.deepgram)
        .await;
    h.state.prewarm_speech(true).await;
    // Maintenance leaves fresh verdicts alone.
    h.state.prewarm_speech(false).await;
    let r = h.send(token_request()).await;
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.json()["mode"], "proxy");
    h.deepgram.verify().await;
}

#[tokio::test]
async fn speak_forwards_to_aura_with_peeks_key_and_passes_the_audio_through() {
    let h = Harness::start().await;
    mount_silicon(&h.iam, ACCESS).await;
    let pcm: Vec<u8> = (0..48_000u32).map(|i| (i % 251) as u8).collect();
    Mock::given(method("POST"))
        .and(path("/v1/speak"))
        .and(header(
            "authorization",
            format!("Token {PEEK_DG_KEY}").as_str(),
        ))
        .and(query_param("model", "aura-2-thalia-en"))
        .and(query_param("encoding", "linear16"))
        .and(query_param("container", "none"))
        .and(query_param("sample_rate", "16000"))
        .and(query_param("mip_opt_out", "true"))
        .and(body_json(json!({"text": "Hello from peek."})))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "audio/l16;rate=16000;channels=1")
                .insert_header("dg-request-id", "dg-req-1")
                .insert_header("dg-char-count", "16")
                .set_body_bytes(pcm.clone()),
        )
        .expect(1)
        .mount(&h.deepgram)
        .await;
    let r = h
        .send(speak_request(&json!({
            "text": "Hello from peek.", "model": "aura-2-thalia-en", "sample_rate": 16000
        })))
        .await;
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.body, pcm);
    assert_eq!(
        r.header("content-type").as_deref(),
        Some("audio/l16;rate=16000;channels=1")
    );
    assert_eq!(r.header("dg-request-id").as_deref(), Some("dg-req-1"));
    assert_eq!(r.header("dg-char-count").as_deref(), Some("16"));
    let sent = Harness::requests(&h.deepgram, "/v1/speak").await;
    let tags: Vec<String> = sent[0]
        .url
        .query_pairs()
        .filter(|(k, _)| k == "tag")
        .map(|(_, v)| v.into_owned())
        .collect();
    assert_eq!(tags, ["peek", "development"]);
    let event = h
        .events()
        .into_iter()
        .find(|e| e["event"] == "speech.proxy")
        .unwrap();
    assert_eq!(event["context"]["dg_request_id"], "dg-req-1");
    assert!(
        !event.to_string().contains("Hello from peek"),
        "text is never recorded"
    );
}

/// A raw HTTP/1.1 server standing in for Deepgram: it sends the first audio
/// chunk, then holds the stream open until the test releases it.
async fn slow_deepgram(
    first: Vec<u8>,
    rest: Vec<u8>,
) -> (String, tokio::sync::oneshot::Sender<()>) {
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
        let mut head = b"HTTP/1.1 200 OK\r\ncontent-type: audio/l16;rate=24000;channels=1\r\ndg-request-id: dg-slow\r\ntransfer-encoding: chunked\r\n\r\n".to_vec();
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

/// A raw HTTP/1.1 server standing in for a hostile "Deepgram": it answers
/// every request with `status_line` and a chunked body that never ends,
/// counting the bytes it managed to write. peek-server must stop reading
/// (and drop the connection) at its cap instead of buffering the body.
async fn endless_deepgram(
    status_line: &'static str,
) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use std::sync::{Arc, atomic::AtomicUsize, atomic::Ordering};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let written = Arc::new(AtomicUsize::new(0));
    let counter = written.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let counter = counter.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 64 * 1024];
                let mut seen = Vec::new();
                loop {
                    let Ok(n) = sock.read(&mut buf).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
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
                let head = format!(
                    "{status_line}\r\ncontent-type: application/json\r\ntransfer-encoding: chunked\r\n\r\n"
                );
                if sock.write_all(head.as_bytes()).await.is_err() {
                    return;
                }
                let payload = vec![b'x'; 64 * 1024];
                let mut chunk = format!("{:x}\r\n", payload.len()).into_bytes();
                chunk.extend_from_slice(&payload);
                chunk.extend_from_slice(b"\r\n");
                // Stop after 512 MiB so a regression fails the assertion
                // below instead of running for minutes.
                for _ in 0..8192 {
                    if sock.write_all(&chunk).await.is_err() {
                        return;
                    }
                    counter.fetch_add(chunk.len(), Ordering::Relaxed);
                }
            });
        }
    });
    (format!("http://{addr}"), written)
}

#[tokio::test]
async fn oversized_upstream_bodies_are_never_buffered() {
    use std::sync::atomic::Ordering;
    // Error bodies (speak's non-200) are read up to a few KiB only.
    let (base, written) = endless_deepgram("HTTP/1.1 500 Internal Server Error").await;
    let h = Harness::with(
        move |env| {
            env.insert("PEEK_DEEPGRAM_BASE_URL", base);
        },
        Limits::default(),
    )
    .await;
    mount_silicon(&h.iam, ACCESS).await;
    let r = tokio::time::timeout(
        Duration::from_secs(20),
        h.send(speak_request(
            &json!({"text": "hi", "model": "aura-2-thalia-en"}),
        )),
    )
    .await
    .expect("an endless error body must not hold the request open");
    assert_eq!(r.status, 503, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.code(), "speech_unavailable");
    assert_eq!(
        r.json()["error"]["details"]["reason"],
        "deepgram_unavailable"
    );
    // A grant (token route) answering 500 with an endless body: same.
    let r = tokio::time::timeout(Duration::from_secs(20), h.send(token_request()))
        .await
        .expect("an endless grant error body must not hold the request open");
    assert_eq!(r.status, 503, "{}", String::from_utf8_lossy(&r.body));
    tokio::time::sleep(Duration::from_millis(200)).await;
    let total = written.load(Ordering::Relaxed);
    assert!(
        total < 64 * 1024 * 1024,
        "peek-server kept reading an endless error body ({total} bytes written)"
    );

    // A 200 transcription larger than the 16 MiB cap is refused while
    // streaming, not after buffering it all.
    let (base, written) = endless_deepgram("HTTP/1.1 200 OK").await;
    let h = Harness::with(
        move |env| {
            env.insert("PEEK_DEEPGRAM_BASE_URL", base);
        },
        Limits::default(),
    )
    .await;
    mount_silicon(&h.iam, ACCESS).await;
    let r = tokio::time::timeout(
        Duration::from_secs(20),
        h.send(listen_request("model=nova-3", vec![0u8; 1024])),
    )
    .await
    .expect("an endless transcription must not hold the request open");
    assert_eq!(r.status, 503, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(
        r.json()["error"]["details"]["reason"],
        "deepgram_unavailable"
    );
    // An endless 200 grant body is malformed, not buffered.
    let r = tokio::time::timeout(Duration::from_secs(20), h.send(token_request()))
        .await
        .expect("an endless grant body must not hold the request open");
    assert_eq!(r.status, 503, "{}", String::from_utf8_lossy(&r.body));
    tokio::time::sleep(Duration::from_millis(200)).await;
    let total = written.load(Ordering::Relaxed);
    assert!(
        total < 96 * 1024 * 1024,
        "peek-server read far past the 16 MiB transcription cap ({total} bytes written)"
    );
}

#[tokio::test]
async fn speak_streams_unbuffered() {
    let first = vec![1u8; 4800];
    let rest = vec![2u8; 9600];
    let (base, release) = slow_deepgram(first.clone(), rest.clone()).await;
    let h = Harness::with(
        move |env| {
            env.insert("PEEK_DEEPGRAM_BASE_URL", base);
        },
        Limits::default(),
    )
    .await;
    mount_silicon(&h.iam, ACCESS).await;
    let response = h
        .app
        .clone()
        .oneshot(speak_request(
            &json!({"text": "A long sentence.", "model": "aura-2-thalia-en"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers()["dg-request-id"].to_str().unwrap(),
        "dg-slow"
    );
    let mut body = response.into_body();
    let mut got = Vec::new();
    // The first chunk arrives while Deepgram is still holding the rest.
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
async fn speak_validates_and_maps_deepgram_failures() {
    let h = Harness::start().await;
    mount_silicon(&h.iam, ACCESS).await;
    for bad in [
        json!({"text": "", "model": "aura-2-thalia-en"}),
        json!({"text": "hi", "model": "nova-3"}),
        json!({"text": "hi", "model": "aura-2-thalia-en", "sample_rate": 44100}),
        json!({"text": "hi", "model": "aura-2-thalia-en", "voice": "x"}),
        json!({"text": "x".repeat(2001), "model": "aura-2-thalia-en"}),
    ] {
        let r = h.send(speak_request(&bad)).await;
        assert_eq!(r.status, 400, "{bad}");
    }
    let unauthenticated = h
        .send(json_body(
            post("/api/v1/speech/speak"),
            &json!({"text": "hi", "model": "aura-2-thalia-en"}),
        ))
        .await;
    assert_eq!(unauthenticated.status, 401);
    Mock::given(method("POST"))
        .and(path("/v1/speak"))
        .respond_with(ResponseTemplate::new(401).insert_header("dg-request-id", "dg-401"))
        .mount(&h.deepgram)
        .await;
    let r = h
        .send(speak_request(
            &json!({"text": "hi", "model": "aura-2-thalia-en"}),
        ))
        .await;
    assert_eq!(r.status, 503);
    assert_eq!(r.code(), "speech_unavailable");
    assert_eq!(r.json()["error"]["details"]["reason"], "peek_key_invalid");
    assert_eq!(r.json()["error"]["details"]["dg_request_id"], "dg-401");
    h.deepgram.reset().await;
    Mock::given(method("POST"))
        .and(path("/v1/speak"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&h.deepgram)
        .await;
    let r = h
        .send(speak_request(
            &json!({"text": "hi", "model": "aura-2-thalia-en"}),
        ))
        .await;
    assert_eq!(r.status, 503);
    assert_eq!(r.json()["error"]["retryable"], true);
    assert!(r.header("retry-after").is_some());
}

#[tokio::test]
async fn listen_forwards_allow_listed_parameters_and_up_to_four_mib() {
    let h = Harness::start().await;
    mount_silicon(&h.iam, ACCESS).await;
    let result = json!({"metadata": {"request_id": "dg-listen"},
        "results": {"channels": [{"alternatives": [{"transcript": "the second one", "confidence": 0.99}]}]}});
    Mock::given(method("POST"))
        .and(path("/v1/listen"))
        .and(header(
            "authorization",
            format!("Token {PEEK_DG_KEY}").as_str(),
        ))
        .and(header("content-type", "audio/wav"))
        .and(query_param("model", "nova-3"))
        .and(query_param("smart_format", "true"))
        .and(query_param("keyterm", "Keep it"))
        .and(query_param("mip_opt_out", "true"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("dg-request-id", "dg-listen")
                .set_body_json(result.clone()),
        )
        .mount(&h.deepgram)
        .await;
    // 2 MiB: above the 1 MiB default body limit, inside the route's 4 MiB.
    let audio = vec![0u8; 2 * 1024 * 1024];
    let r = h
        .send(listen_request(
            "model=nova-3&smart_format=true&keyterm=Keep+it&keyterm=Delete&detect_language=en&detect_language=hi",
            audio.clone(),
        ))
        .await;
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.json(), result);
    assert_eq!(r.header("dg-request-id").as_deref(), Some("dg-listen"));
    let sent = Harness::requests(&h.deepgram, "/v1/listen").await;
    assert_eq!(sent[0].body.len(), audio.len());
    let pairs: Vec<(String, String)> = sent[0]
        .url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    assert_eq!(
        pairs.iter().filter(|(k, _)| k == "keyterm").count(),
        2,
        "{pairs:?}"
    );
    assert_eq!(
        pairs.iter().filter(|(k, _)| k == "detect_language").count(),
        2
    );
    assert!(pairs.contains(&("tag".to_owned(), "peek".to_owned())));

    let refused = h
        .send(listen_request(
            "model=nova-3&callback=https://evil",
            vec![1; 10],
        ))
        .await;
    assert_eq!(refused.status, 400);
    assert!(
        refused.json()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("callback")
    );
    let too_big = h
        .send(listen_request(
            "model=nova-3",
            vec![0u8; 4 * 1024 * 1024 + 1],
        ))
        .await;
    assert_eq!(too_big.status, 413);
    let not_audio = h
        .send(
            authed("POST", "/api/v1/speech/listen?model=nova-3", ACCESS)
                .header("content-type", "application/json")
                .header("idempotency-key", IDEM)
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await;
    assert_eq!(not_audio.status, 415);
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
        .and(path("/v1/speak"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "audio/l16;rate=24000")
                .set_body_bytes(vec![0u8; 10]),
        )
        .mount(&h.deepgram)
        .await;
    let body = json!({"text": "hi", "model": "aura-2-thalia-en"});
    assert_eq!(h.send(speak_request(&body)).await.status, 200);
    let r = h.send(speak_request(&body)).await;
    assert_eq!(r.status, 429);
    assert_eq!(r.code(), "rate_limited");
}
