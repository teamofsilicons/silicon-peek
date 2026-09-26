//! Deepgram JWT minting and org BYO keys against a fake Deepgram.

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

const BYO_KEY: &str = "dg-org-own-key-0123456789";

fn token_request(purpose: &str) -> Request<Body> {
    json_body(
        authed("POST", "/api/v1/speech/token", ACCESS)
            .header("content-type", "application/json")
            .header("idempotency-key", IDEM),
        &json!({"purpose": purpose}),
    )
}

async fn mount_grant(h: &Harness, key: &str, template: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path("/v1/auth/grant"))
        .and(header("authorization", format!("Token {key}").as_str()))
        .respond_with(template)
        .mount(&h.deepgram)
        .await;
}

fn grant_ok(jwt: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"access_token": jwt, "expires_in": 60}))
}

fn byo_put(body: &serde_json::Value) -> Request<Body> {
    json_body(
        authed("PUT", "/api/v1/orgs/tos/byo/deepgram", ACCESS)
            .header("content-type", "application/json"),
        body,
    )
}

#[tokio::test]
async fn mints_with_peeks_key() {
    let h = Harness::start().await;
    mount_silicon(&h.iam, ACCESS).await;
    Mock::given(method("POST"))
        .and(path("/v1/auth/grant"))
        .and(header(
            "authorization",
            format!("Token {PEEK_DG_KEY}").as_str(),
        ))
        .and(body_json(json!({"ttl_seconds": 60})))
        .respond_with(grant_ok("jwt-peek"))
        .expect(1)
        .mount(&h.deepgram)
        .await;
    let r = h.send(token_request("tts")).await;
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.header("cache-control").as_deref(), Some("no-store"));
    assert_eq!(
        r.json(),
        json!({"mode": "direct", "access_token": "jwt-peek", "expires_in": 60, "base_url": h.deepgram.uri(), "key_source": "peek",
               "params": {"mip_opt_out": true, "tags": ["peek", "development"]}})
    );
    let event = h
        .events()
        .into_iter()
        .find(|e| e["event"] == "speech.token")
        .unwrap();
    assert_eq!(event["step"], "speech.tts");
    assert_eq!(event["context"]["key_source"], "peek");
    assert!(
        !event.to_string().contains("jwt-peek"),
        "JWTs are never recorded"
    );
}

#[tokio::test]
async fn no_key_is_speech_unavailable_not_configured() {
    let h = Harness::with(
        |env| {
            env.remove("PEEK_DEEPGRAM_API_KEY");
        },
        Limits::default(),
    )
    .await;
    mount_silicon(&h.iam, ACCESS).await;
    let r = h.send(token_request("stt")).await;
    assert_eq!(r.status, 503);
    assert_eq!(r.code(), "speech_unavailable");
    assert_eq!(r.json()["error"]["details"]["reason"], "not_configured");
    assert_eq!(r.json()["error"]["retryable"], false);
}

#[tokio::test]
async fn byo_requires_an_admin_and_a_valid_member_key() {
    let h = Harness::start().await;
    mount_introspect(
        &h.iam,
        "oat_memberOnly01",
        introspection(ACTOR, ORG, &FULL_SCOPES, Some("member"), None),
    )
    .await;
    let r = h
        .send(json_body(
            authed("PUT", "/api/v1/orgs/tos/byo/deepgram", "oat_memberOnly01")
                .header("content-type", "application/json"),
            &json!({"api_key": BYO_KEY}),
        ))
        .await;
    assert_eq!(r.status, 403);
    assert_eq!(r.code(), "not_org_admin");

    mount_silicon(&h.iam, ACCESS).await;
    // Deepgram says the key is invalid.
    Mock::given(method("GET"))
        .and(path("/v1/auth/token"))
        .and(header("authorization", "Token dg-bad-key-000000000000"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&h.deepgram)
        .await;
    let r = h
        .send(byo_put(&json!({"api_key": "dg-bad-key-000000000000"})))
        .await;
    assert_eq!(r.status, 400);
    assert_eq!(r.json()["error"]["details"]["reason"], "key_invalid");

    // A valid key that may not mint JWTs is accepted: speech then goes
    // through the proxy (`{"mode":"proxy"}`).
    Mock::given(method("GET"))
        .and(path("/v1/auth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"api_key_id": "k"})))
        .mount(&h.deepgram)
        .await;
    mount_grant(&h, "dg-viewer-key-00000000", ResponseTemplate::new(403)).await;
    let r = h
        .send(byo_put(&json!({"api_key": "dg-viewer-key-00000000"})))
        .await;
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.json()["configured"], true);
    // A project without credits is refused.
    mount_grant(&h, "dg-broke-key-000000000", ResponseTemplate::new(402)).await;
    let r = h
        .send(byo_put(&json!({"api_key": "dg-broke-key-000000000"})))
        .await;
    assert_eq!(r.status, 400);
    assert_eq!(r.json()["error"]["details"]["reason"], "out_of_credits");

    // Another org in the path.
    let r = h
        .send(json_body(
            authed("PUT", "/api/v1/orgs/acme/byo/deepgram", ACCESS)
                .header("content-type", "application/json"),
            &json!({"api_key": BYO_KEY}),
        ))
        .await;
    assert_eq!(r.status, 400);
}

#[tokio::test]
async fn byo_base_urls_are_limited_to_allowed_https_hosts() {
    let h = Harness::start().await;
    mount_silicon(&h.iam, ACCESS).await;
    // Refused before any upstream is contacted: http, loopback, IP literals
    // (cloud metadata, private ranges) and hosts outside the allowlist.
    for bad in [
        "http://127.0.0.1:8080",
        "https://127.0.0.1:8080",
        "https://localhost:8443",
        "https://169.254.169.254",
        "https://[::1]",
        "https://attacker.example",
        "https://api.deepgram.com.attacker.example",
    ] {
        let r = h
            .send(byo_put(&json!({"api_key": BYO_KEY, "base_url": bad})))
            .await;
        assert_eq!(r.status, 400, "{bad}: {}", String::from_utf8_lossy(&r.body));
        assert_eq!(
            r.json()["error"]["details"]["reason"],
            "base_url_not_allowed",
            "{bad}"
        );
    }
    assert!(
        Harness::requests(&h.deepgram, "/v1/auth/token")
            .await
            .is_empty(),
        "a refused base URL is never called"
    );

    // A base URL stored before the policy (or before the operator narrowed
    // it) is refused on use, never silently replaced by peek's key.
    Mock::given(method("GET"))
        .and(path("/v1/auth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&h.deepgram)
        .await;
    mount_grant(&h, BYO_KEY, grant_ok("jwt-probe")).await;
    let r = h.send(byo_put(&json!({"api_key": BYO_KEY}))).await;
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let db = rusqlite::Connection::open(h.dir.path().join("peek.sqlite")).unwrap();
    db.execute(
        "UPDATE byo_keys SET base_url = 'http://127.0.0.1:9' WHERE org_id = 'tos'",
        [],
    )
    .unwrap();
    drop(db);
    let r = h.send(token_request("tts")).await;
    assert_eq!(r.status, 503, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(
        r.json()["error"]["details"]["reason"],
        "org_base_url_not_allowed"
    );
    assert_eq!(r.json()["error"]["retryable"], false);
    let hint = r.json()["error"]["hint"].as_str().unwrap().to_owned();
    assert!(
        hint.contains("peek --org tos org byo deepgram set --key-file -")
            && hint.contains("peek --org tos org byo deepgram delete"),
        "{hint}"
    );
}

#[tokio::test]
async fn byo_keys_are_used_without_fallback_and_never_returned() {
    let h = Harness::start().await;
    mount_silicon(&h.iam, ACCESS).await;
    Mock::given(method("GET"))
        .and(path("/v1/auth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&h.deepgram)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/auth/grant"))
        .and(header("authorization", format!("Token {BYO_KEY}").as_str()))
        .and(body_json(json!({"ttl_seconds": 1})))
        .respond_with(grant_ok("jwt-probe"))
        .mount(&h.deepgram)
        .await;
    let r = h.send(byo_put(&json!({"api_key": BYO_KEY}))).await;
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.json()["configured"], true);
    assert!(r.json()["updated_at"].is_string());
    assert!(
        !String::from_utf8_lossy(&r.body).contains(BYO_KEY),
        "the key is never returned"
    );
    let status = h
        .send(
            authed("GET", "/api/v1/orgs/tos/byo/deepgram", ACCESS)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(status.json()["configured"], true);
    assert!(!String::from_utf8_lossy(&status.body).contains(BYO_KEY));
    for file in ["peek.sqlite", "peek.sqlite-wal"] {
        let bytes = std::fs::read(h.dir.path().join(file)).unwrap_or_default();
        assert!(
            !String::from_utf8_lossy(&bytes).contains(BYO_KEY),
            "sealed at rest"
        );
    }

    // Minting uses the org's key.
    mount_grant(&h, BYO_KEY, grant_ok("jwt-org")).await;
    let r = h.send(token_request("stt")).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json()["key_source"], "org");
    assert_eq!(r.json()["access_token"], "jwt-org");

    // The org's key fails: no silent fallback to peek's key.
    for (status, reason) in [(402, "org_out_of_credits"), (429, "rate_limited")] {
        h.deepgram.reset().await;
        mount_grant(&h, BYO_KEY, ResponseTemplate::new(status)).await;
        mount_grant(&h, PEEK_DG_KEY, grant_ok("jwt-peek")).await;
        let r = h.send(token_request("tts")).await;
        assert_eq!(r.status, 503, "{reason}");
        assert_eq!(r.code(), "speech_unavailable");
        assert_eq!(r.json()["error"]["details"]["reason"], reason);
        let used_peek = Harness::requests(&h.deepgram, "/v1/auth/grant")
            .await
            .iter()
            .any(|req| req.headers["authorization"] == format!("Token {PEEK_DG_KEY}").as_str());
        assert!(!used_peek, "{reason}: never falls back to peek's key");
    }
    // An org key that may not mint (401/403) is served through the proxy
    // with the org's key, never peek's.
    h.deepgram.reset().await;
    mount_grant(&h, BYO_KEY, ResponseTemplate::new(403)).await;
    mount_grant(&h, PEEK_DG_KEY, grant_ok("jwt-peek")).await;
    let r = h.send(token_request("tts")).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json()["mode"], "proxy");
    assert_eq!(r.json()["key_source"], "org");
    assert!(r.json().get("access_token").is_none());

    // Removing the key restores peek's key.
    let r = h
        .send(
            authed("DELETE", "/api/v1/orgs/tos/byo/deepgram", ACCESS)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, 204);
    let r = h.send(token_request("tts")).await;
    assert_eq!(r.json()["key_source"], "peek");
}

#[tokio::test]
async fn minting_is_rate_limited_per_actor() {
    let h = Harness::with(
        |_| {},
        Limits {
            speech_per_actor_per_minute: 2,
            ..Limits::default()
        },
    )
    .await;
    mount_silicon(&h.iam, ACCESS).await;
    mount_grant(&h, PEEK_DG_KEY, grant_ok("jwt")).await;
    assert_eq!(h.send(token_request("tts")).await.status, 200);
    assert_eq!(h.send(token_request("tts")).await.status, 200);
    let r = h.send(token_request("tts")).await;
    assert_eq!(r.status, 429);
    assert_eq!(r.code(), "rate_limited");
    assert!(r.header("retry-after").is_some());
}

#[tokio::test]
async fn deepgram_outages_are_retryable() {
    let h = Harness::start().await;
    mount_silicon(&h.iam, ACCESS).await;
    mount_grant(&h, PEEK_DG_KEY, ResponseTemplate::new(503)).await;
    let r = h.send(token_request("tts")).await;
    assert_eq!(r.status, 503);
    assert_eq!(
        r.json()["error"]["details"]["reason"],
        "deepgram_unavailable"
    );
    assert_eq!(r.json()["error"]["retryable"], true);
}
