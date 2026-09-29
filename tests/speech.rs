//! Legacy Deepgram BYO administration stays usable but never selects a speech provider.
#![allow(clippy::unwrap_used, clippy::expect_used)]
mod common;
use axum::body::Body;
use common::*;
use serde_json::json;
use wiremock::{
    Mock, ResponseTemplate,
    matchers::{method, path},
};

const BYO_KEY: &str = "dg-org-own-key-0123456789";

async fn allow_legacy_validation(h: &Harness) {
    for (verb, route) in [("GET", "/v1/auth/token"), ("POST", "/v1/auth/grant")] {
        Mock::given(method(verb))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200))
            .mount(&h.deepgram)
            .await;
    }
}

#[tokio::test]
async fn legacy_byo_is_sealed_and_does_not_override_openai() {
    let h = Harness::start().await;
    mount_silicon(&h.iam, ACCESS).await;
    allow_legacy_validation(&h).await;
    let r = h
        .send(json_body(
            authed("PUT", "/api/v1/orgs/tos/byo/deepgram", ACCESS)
                .header("content-type", "application/json"),
            &json!({"api_key":BYO_KEY}),
        ))
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json()["configured"], true);
    for file in ["peek.sqlite", "peek.sqlite-wal"] {
        assert!(
            !String::from_utf8_lossy(&std::fs::read(h.dir.path().join(file)).unwrap_or_default())
                .contains(BYO_KEY)
        );
    }
    let r = h
        .send(
            authed("GET", "/api/v1/orgs/tos/byo/deepgram", ACCESS)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(r.json()["configured"], true);
    assert!(!String::from_utf8_lossy(&r.body).contains(BYO_KEY));
    h.deepgram.reset().await;
    let r = h
        .send(json_body(
            authed("POST", "/api/v1/speech/token", ACCESS)
                .header("content-type", "application/json")
                .header("idempotency-key", IDEM),
            &json!({"purpose":"stt"}),
        ))
        .await;
    assert_eq!(r.json()["provider"], "openai");
    assert_eq!(r.json()["key_source"], "peek");
    h.state.maintenance().await;
    assert!(h.deepgram.received_requests().await.unwrap().is_empty());
    let r = h
        .send(
            authed("DELETE", "/api/v1/orgs/tos/byo/deepgram", ACCESS)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, 204);
}

#[tokio::test]
async fn legacy_byo_requires_admin_valid_key_and_allowed_origin() {
    let h = Harness::start().await;
    mount_introspect(
        &h.iam,
        ACCESS,
        introspection(ACTOR, ORG, &FULL_SCOPES, Some("member"), None),
    )
    .await;
    let put = |body| {
        json_body(
            authed("PUT", "/api/v1/orgs/tos/byo/deepgram", ACCESS)
                .header("content-type", "application/json"),
            &body,
        )
    };
    assert_eq!(h.send(put(json!({"api_key":BYO_KEY}))).await.status, 403);
    h.iam.reset().await;
    mount_silicon(&h.iam, ACCESS).await;
    for origin in [
        "http://127.0.0.1:9",
        "https://api.deepgram.com.attacker.example",
        "https://u:p@api.deepgram.com",
    ] {
        assert_eq!(
            h.send(put(json!({"api_key":BYO_KEY,"base_url":origin})))
                .await
                .status,
            400
        );
    }
    assert!(h.deepgram.received_requests().await.unwrap().is_empty());
    Mock::given(method("GET"))
        .and(path("/v1/auth/token"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&h.deepgram)
        .await;
    let r = h.send(put(json!({"api_key":BYO_KEY}))).await;
    assert_eq!(r.status, 400);
    assert_eq!(r.json()["error"]["details"]["reason"], "key_invalid");
}
