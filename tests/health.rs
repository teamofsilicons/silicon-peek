//! Health, readiness, discovery and the cross-cutting HTTP contract
//! (request IDs, envelopes, caching, body limits).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use axum::{body::Body, http::Request};
use common::{Harness, json_body, post};
use serde_json::json;
use silicon_peek::state::Limits;

#[tokio::test]
async fn healthz_and_request_ids() {
    let h = Harness::start().await;
    let r = h
        .send(Request::get("/healthz").body(Body::empty()).unwrap())
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(
        r.json(),
        json!({"status": "ok", "service": "peek", "version": env!("CARGO_PKG_VERSION")})
    );
    let id = r.header("x-request-id").expect("x-request-id");
    assert_eq!(id.len(), 36);
    assert_eq!(r.header("cache-control").as_deref(), Some("no-store"));
    let again = h
        .send(Request::get("/healthz").body(Body::empty()).unwrap())
        .await;
    assert_ne!(
        again.header("x-request-id"),
        Some(id),
        "a fresh ID per request"
    );
}

#[tokio::test]
async fn readyz_reports_every_dependency() {
    let h = Harness::start().await;
    let r = h
        .send(Request::get("/readyz").body(Body::empty()).unwrap())
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(
        r.json(),
        json!({"status": "ready", "checks": {"db": "ok", "iam_config": "ok", "ting_config": "ok", "deepgram": "configured", "gemini": "configured", "openai": "configured"}})
    );
}

#[tokio::test]
async fn an_empty_app_secret_is_allowed_but_not_ready() {
    let h = Harness::with(
        |env| {
            env.remove("PEEK_IAM_APP_SECRET");
            env.remove("PEEK_DEEPGRAM_API_KEY");
        },
        Limits::default(),
    )
    .await;
    let r = h
        .send(Request::get("/readyz").body(Body::empty()).unwrap())
        .await;
    assert_eq!(r.status, 503);
    assert_eq!(r.json()["status"], "not_ready");
    assert_eq!(r.json()["checks"]["iam_config"], "missing");
    assert_eq!(r.json()["checks"]["deepgram"], "missing");

    let login = h
        .send(json_body(
            post("/api/v1/auth/login"),
            &json!({"slt": "oac_abc"}),
        ))
        .await;
    assert_eq!(login.status, 503);
    assert_eq!(login.code(), "iam_misconfigured");
    assert_eq!(login.json()["error"]["retryable"], true);
    assert!(
        h.iam
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty(),
        "no IAM call without a secret"
    );
}

#[tokio::test]
async fn discovery_in_production() {
    let h = Harness::start().await;
    let r = h
        .send(Request::get("/api/v1/iam").body(Body::empty()).unwrap())
        .await;
    assert_eq!(r.status, 200);
    let v = r.json();
    assert_eq!(v["app_id"], "peek");
    assert_eq!(v["api_version"], "v1");
    assert_eq!(v["api_base_url"], "https://backend.peek.teamofsilicons.com");
    assert_eq!(v["iam_base_url"], h.iam.uri());
    assert!(v["testing_environment_id"].is_null());
    assert!(v["testing_generation"].is_null());
    assert!(v["testing_environment"].is_null());
    assert_eq!(
        v["compatibility"],
        json!({"cli": ">=0.1.0, <1.0.0", "ipc_protocols": [1]})
    );
}

#[tokio::test]
async fn unknown_routes_methods_and_bad_paths_use_the_envelope() {
    let h = Harness::start().await;
    let r = h
        .send(Request::get("/nope").body(Body::empty()).unwrap())
        .await;
    assert_eq!(r.status, 404);
    assert_eq!(r.code(), "not_found");
    assert_eq!(
        r.json()["error"]["request_id"].as_str(),
        r.header("x-request-id").as_deref()
    );

    let r = h
        .send(Request::delete("/healthz").body(Body::empty()).unwrap())
        .await;
    assert_eq!(r.status, 405);
    assert_eq!(r.code(), "invalid_input");

    let r = h
        .send(
            Request::get("/internal/honeycomb/organizations/tos/testing-environments/not-a-uuid/operations/x")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, 400);
    assert_eq!(r.code(), "invalid_input");
}

#[tokio::test]
async fn posts_need_an_idempotency_key_and_strict_json() {
    let h = Harness::start().await;
    let r = h
        .send(
            Request::post("/api/v1/auth/login")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"slt":"oac_x"}"#))
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, 400);
    assert_eq!(r.code(), "idempotency_key_required");

    let r = h
        .send(json_body(
            post("/api/v1/auth/login"),
            &json!({"slt": "oac_x", "org": "tos"}),
        ))
        .await;
    assert_eq!(r.status, 400);
    assert_eq!(r.code(), "invalid_input", "unknown fields are refused");

    let r = h
        .send(
            post("/api/v1/auth/login")
                .body(Body::from(r#"{"slt":"a","slt":"b"}"#))
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, 400);
    assert_eq!(r.code(), "invalid_json", "duplicate keys are refused");

    let r = h
        .send(
            Request::post("/api/v1/auth/login")
                .header("idempotency-key", "short")
                .body(Body::from(r#"{"slt":"a"}"#))
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, 400);
    assert_eq!(r.code(), "invalid_input");
}

#[tokio::test]
async fn bodies_over_one_mebibyte_are_refused() {
    let h = Harness::start().await;
    let big = "x".repeat(1024 * 1024 + 10);
    let r = h
        .send(
            post("/api/v1/reports")
                .body(Body::from(format!(r#"{{"message":"{big}"}}"#)))
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, 413);
    assert_eq!(r.code(), "payload_too_large");
}

#[tokio::test]
async fn http_completed_telemetry_skips_health_and_honours_opt_out() {
    let h = Harness::start().await;
    h.send(Request::get("/healthz").body(Body::empty()).unwrap())
        .await;
    h.send(Request::get("/api/v1/iam").body(Body::empty()).unwrap())
        .await;
    h.send(
        Request::get("/api/v1/iam")
            .header("x-peek-telemetry", "off")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let events = h.events();
    assert_eq!(events.len(), 1, "{events:?}");
    let e = &events[0];
    assert_eq!(e["event"], "http.completed");
    assert_eq!(e["service"], "peek-backend");
    assert_eq!(e["environment"], "development");
    assert_eq!(e["context"]["http_route"], "/api/v1/iam");
    assert_eq!(e["context"]["status"], 200);
}
