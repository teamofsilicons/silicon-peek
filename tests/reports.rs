//! `POST /api/v1/reports` against a fake GitHub.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use axum::{body::Body, http::Request};
use common::*;
use serde_json::{Value, json};
use silicon_peek::state::Limits;
use wiremock::{
    Mock, ResponseTemplate,
    matchers::{header, method, path},
};

const ISSUES: &str = "/repos/teamofsilicons/silicon-peek/issues";

fn report(body: &Value, key: &str) -> Request<Body> {
    Request::post("/api/v1/reports")
        .header("content-type", "application/json")
        .header("idempotency-key", key)
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn sample() -> Value {
    json!({
        "message": "send hangs when Peek.app is closed\nsteps: quit the app, run peek send --speak hi",
        "pr": "https://github.com/teamofsilicons/silicon-peek/pull/12",
        "context": {"cli_version": "0.1.0", "platform": "macos-aarch64", "command": "send", "error_code": "daemon_unavailable"}
    })
}

#[tokio::test]
async fn files_a_github_issue_once() {
    let h = Harness::start().await;
    Mock::given(method("POST"))
        .and(path(ISSUES))
        .and(header(
            "authorization",
            format!("Bearer {GITHUB_TOKEN}").as_str(),
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "html_url": "https://github.com/teamofsilicons/silicon-peek/issues/7"
        })))
        .expect(1)
        .mount(&h.github)
        .await;
    let r = h.send(report(&sample(), "peek-report-000000000001")).await;
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let v = r.json();
    assert!(v["id"].as_str().unwrap().starts_with("rep_"));
    assert_eq!(v["status"], "filed");
    assert_eq!(
        v["issue_url"],
        "https://github.com/teamofsilicons/silicon-peek/issues/7"
    );

    let issue: Value =
        serde_json::from_slice(&Harness::requests(&h.github, ISSUES).await[0].body).unwrap();
    assert_eq!(issue["title"], "send hangs when Peek.app is closed");
    let body = issue["body"].as_str().unwrap();
    assert!(body.contains("Proposed fix: https://github.com/teamofsilicons/silicon-peek/pull/12"));
    assert!(body.contains("Submitted with peek 0.1.0 on macos-aarch64"));

    let again = h.send(report(&sample(), "peek-report-000000000001")).await;
    assert_eq!(again.json(), v, "a retry replays; no second issue");
    let event = h
        .events()
        .into_iter()
        .find(|e| e["event"] == "report.created")
        .unwrap();
    assert_eq!(event["context"]["status"], "filed");
}

#[tokio::test]
async fn reports_are_stored_when_github_is_unavailable_or_unconfigured() {
    let h = Harness::start().await;
    Mock::given(method("POST"))
        .and(path(ISSUES))
        .respond_with(ResponseTemplate::new(503))
        .mount(&h.github)
        .await;
    let r = h
        .send(report(
            &json!({"message": "it broke"}),
            "peek-report-000000000002",
        ))
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json()["status"], "stored");
    assert!(r.json()["issue_url"].is_null());

    let no_token = Harness::with(
        |env| {
            env.remove("PEEK_GITHUB_ISSUES_TOKEN");
        },
        Limits::default(),
    )
    .await;
    let r = no_token
        .send(report(
            &json!({"message": "it broke"}),
            "peek-report-000000000003",
        ))
        .await;
    assert_eq!(r.json()["status"], "stored");
    assert!(
        no_token
            .github
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty()
    );
}

#[tokio::test]
async fn reports_are_validated_and_rate_limited() {
    let h = Harness::with(
        |_| {},
        Limits {
            reports_per_ip_per_hour: 2,
            ..Limits::default()
        },
    )
    .await;
    Mock::given(method("POST"))
        .and(path(ISSUES))
        .respond_with(
            ResponseTemplate::new(201)
                .set_body_json(json!({"html_url": "https://github.com/x/y/issues/1"})),
        )
        .mount(&h.github)
        .await;
    let r = h
        .send(report(
            &json!({"message": "x", "pr": "https://github.com/other/repo/pull/1"}),
            "peek-report-000000000004",
        ))
        .await;
    assert_eq!(r.status, 400);
    let r = h
        .send(report(
            &json!({"message": "  "}),
            "peek-report-000000000005",
        ))
        .await;
    assert_eq!(r.status, 400);
    let r = h
        .send(report(
            &json!({"message": "x", "extra": 1}),
            "peek-report-000000000006",
        ))
        .await;
    assert_eq!(r.status, 400);

    assert_eq!(
        h.send(report(
            &json!({"message": "one"}),
            "peek-report-000000000007"
        ))
        .await
        .status,
        200
    );
    assert_eq!(
        h.send(report(
            &json!({"message": "two"}),
            "peek-report-000000000008"
        ))
        .await
        .status,
        200
    );
    let r = h
        .send(report(
            &json!({"message": "three"}),
            "peek-report-000000000009",
        ))
        .await;
    assert_eq!(r.status, 429);
    assert_eq!(r.code(), "rate_limited");
    let r = h
        .send(report(
            &json!({"message": "one"}),
            "peek-report-000000000007",
        ))
        .await;
    assert_eq!(
        r.status, 200,
        "replays of finished reports are not rate limited"
    );
}

#[tokio::test]
async fn a_bearer_attributes_the_report_but_a_bad_one_never_blocks_it() {
    let h = Harness::start().await;
    mount_introspect(&h.iam, "oat_revokedToken01", json!({"active": false})).await;
    let r = h
        .send(json_body(
            authed("POST", "/api/v1/reports", "oat_revokedToken01")
                .header("content-type", "application/json")
                .header("idempotency-key", "peek-report-000000000010"),
            &json!({"message": "login status says rejected"}),
        ))
        .await;
    assert_eq!(r.status, 200);
}
