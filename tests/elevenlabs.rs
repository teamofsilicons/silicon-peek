//! Short-lived direct TTS credentials, isolation and bounded provider responses.
#![allow(clippy::unwrap_used, clippy::expect_used)]
mod common;
use axum::{body::Body, http::Request};
use common::*;
use serde_json::json;
use silicon_peek::state::Limits;
use wiremock::{
    Mock, ResponseTemplate,
    matchers::{body_json, header, method, path},
};
const JWT: &str = "temporary-deepgram-credential-for-client-connection";

fn token(test: bool) -> Request<Body> {
    let b = authed("POST", "/api/v1/speech/token", ACCESS)
        .header("content-type", "application/json")
        .header("idempotency-key", IDEM);
    json_body(
        if test { testing(b, Some(1)) } else { b },
        &json!({"purpose":"tts"}),
    )
}
async fn grant(h: &Harness, key: &str) {
    Mock::given(method("POST"))
        .and(path("/v1/auth/grant"))
        .and(header("authorization", format!("Token {key}").as_str()))
        .and(body_json(json!({"ttl_seconds":30})))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"access_token":JWT,"expires_in":30})),
        )
        .mount(&h.deepgram)
        .await;
}

#[tokio::test]
async fn tts_mints_a_fresh_direct_token_and_never_relays_audio() {
    let h = Harness::start().await;
    mount_silicon(&h.iam, ACCESS).await;
    grant(&h, PEEK_DG_KEY).await;
    for _ in 0..2 {
        let r = h.send(token(false)).await;
        assert_eq!(r.status, 200);
        assert_eq!(
            r.json(),
            json!({"provider":"elevenlabs","mode":"direct","access_token":JWT,"expires_in":30,
            "base_url":"wss://agent.deepgram.com/v1/agent/converse","key_source":"peek","params":{"mip_opt_out":true,"tags":[]}})
        );
        assert_eq!(r.header("cache-control").as_deref(), Some("no-store"));
    }
    assert_eq!(
        Harness::requests(&h.deepgram, "/v1/auth/grant").await.len(),
        2,
        "token requests are never replayed"
    );
    assert!(Harness::requests(&h.deepgram, "/v1/speak").await.is_empty());
    let obsolete = json_body(
        authed("POST", "/api/v1/speech/speak", ACCESS)
            .header("content-type", "application/json")
            .header("idempotency-key", IDEM),
        &json!({"text":"hello","model":"JBFqnCBsd6RMkjVDRZzb"}),
    );
    assert_eq!(h.send(obsolete).await.status, 404);
    let events = serde_json::to_string(&h.events()).unwrap();
    assert!(!events.contains(PEEK_DG_KEY));
    assert!(!events.contains(JWT));
    h.deepgram.reset().await;
    h.state.maintenance().await;
    assert!(h.deepgram.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn credentials_never_cross_planes_or_fall_back() {
    for (test, configured) in [(false, false), (true, false), (true, true)] {
        let h = Harness::with(
            |env| {
                if !configured {
                    env.remove(if test {
                        "PEEK_DEEPGRAM_TEST_API_KEY"
                    } else {
                        "PEEK_DEEPGRAM_API_KEY"
                    });
                }
            },
            Limits::default(),
        )
        .await;
        if test {
            mount_testing_contexts(&h.iam).await;
            h.prepare_environment().await;
            mount_introspect_testing(
                &h.iam,
                ACCESS,
                introspection(ACTOR, ORG, &FULL_SCOPES, Some("admin"), Some(env_id())),
            )
            .await;
        } else {
            mount_silicon(&h.iam, ACCESS).await;
        }
        grant(&h, PEEK_DG_TEST_KEY).await;
        let r = h.send(token(test)).await;
        assert_eq!(r.status.as_u16(), if configured { 200 } else { 503 });
        if !configured {
            assert_eq!(r.json()["error"]["details"]["reason"], "not_configured");
        }
        assert_eq!(
            Harness::requests(&h.deepgram, "/v1/auth/grant").await.len(),
            usize::from(configured)
        );
    }
}

#[tokio::test]
async fn requires_bearer_and_shares_the_speech_budget() {
    let h = Harness::with(
        |_| {},
        Limits {
            speech_per_actor_per_minute: 1,
            ..Limits::default()
        },
    )
    .await;
    mount_silicon(&h.iam, ACCESS).await;
    grant(&h, PEEK_DG_KEY).await;
    let mut unauthed = token(false);
    unauthed.headers_mut().remove("authorization");
    assert_eq!(h.send(unauthed).await.status, 401);
    assert!(h.deepgram.received_requests().await.unwrap().is_empty());
    assert_eq!(h.send(token(false)).await.status, 200);
    assert_eq!(h.send(token(false)).await.status, 429);
    assert_eq!(
        Harness::requests(&h.deepgram, "/v1/auth/grant").await.len(),
        1
    );
}

#[tokio::test]
async fn provider_errors_do_not_echo_keys_and_permission_failures_do_not_fall_back() {
    let h = Harness::start().await;
    mount_silicon(&h.iam, ACCESS).await;
    for (status, retry) in [
        (400, false),
        (401, false),
        (403, false),
        (408, true),
        (429, true),
        (500, true),
    ] {
        h.deepgram.reset().await;
        Mock::given(method("POST"))
            .and(path("/v1/auth/grant"))
            .respond_with(
                ResponseTemplate::new(status)
                    .insert_header("retry-after", "8")
                    .set_body_string("private key and response"),
            )
            .mount(&h.deepgram)
            .await;
        let r = h.send(token(false)).await;
        assert_eq!(r.status, 503);
        assert_eq!(r.json()["error"]["retryable"], retry);
        assert_eq!(r.json()["error"]["details"]["deepgram_status"], status);
        assert!(!String::from_utf8_lossy(&r.body).contains("private"));
        if retry {
            assert_eq!(r.header("retry-after").as_deref(), Some("8"));
        }
        assert_eq!(h.deepgram.received_requests().await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn malformed_oversized_or_unusable_credentials_are_refused() {
    let h = Harness::start().await;
    mount_silicon(&h.iam, ACCESS).await;
    for body in [
        "not json".to_owned(),
        "x".repeat(32 * 1024 + 1),
        json!({"access_token":JWT}).to_string(),
        json!({"access_token":JWT,"expires_in":0}).to_string(),
        json!({"access_token":JWT,"expires_in":31}).to_string(),
        json!({"access_token":"","expires_in":30}).to_string(),
        json!({"access_token":"secret\r\nheader-injection","expires_in":30}).to_string(),
        json!({"access_token":"a".repeat(16*1024+1),"expires_in":30}).to_string(),
    ] {
        h.deepgram.reset().await;
        Mock::given(method("POST"))
            .and(path("/v1/auth/grant"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
            .mount(&h.deepgram)
            .await;
        let r = h.send(token(false)).await;
        assert_eq!(r.status, 503);
        assert_eq!(
            r.json()["error"]["details"]["reason"],
            "elevenlabs_unavailable"
        );
        assert!(!String::from_utf8_lossy(&r.body).contains(JWT));
    }
}

#[tokio::test]
async fn a_token_service_transport_failure_is_retryable() {
    let h = Harness::with(
        |env| {
            env.insert("PEEK_DEEPGRAM_BASE_URL", "http://127.0.0.1:9".to_owned());
        },
        Limits::default(),
    )
    .await;
    mount_silicon(&h.iam, ACCESS).await;
    let r = h.send(token(false)).await;
    assert_eq!(r.status, 503);
    assert_eq!(r.json()["error"]["retryable"], true);
}
