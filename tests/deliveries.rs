//! `POST /api/v1/deliveries` and `POST /api/v1/ting/recipient` against fake
//! IAM and Ting: the deterministic body, one fresh proof per attempt, the
//! retry on proof errors, the §3.6 error matrix, idempotency and D7.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use axum::{body::Body, http::Request};
use common::*;
use serde_json::{Value, json};
use silicon_peek_client::{
    identity::{ActorId, DataContext, OrgId, SlotIndex},
    ids::{AskId, SendId},
    schema::ask::{Answer, AskType},
    timestamp::Timestamp,
    ting::{
        AnswerVia, AskAnswered, DeliveryRequest, SchemaV1, TingData, TingMetadata, ting_send_body,
    },
};
use wiremock::{
    Mock, ResponseTemplate,
    matchers::{method, path},
};

fn delivery(context: DataContext) -> DeliveryRequest {
    let data = TingData::AskAnswered(AskAnswered {
        schema: SchemaV1,
        ask_id: AskId::generate(),
        send_id: SendId::generate(),
        question: "Delete ~/Downloads/old.zip?".into(),
        ask_type: AskType::SingleChoice,
        answer: Answer::SingleChoice {
            option_id: "keep".into(),
            label: "Keep it".into(),
        },
        via: AnswerVia::Click,
        transcript: None,
        asked_at: Timestamp::parse("2026-09-26T10:00:00Z").unwrap(),
        answered_at: Timestamp::parse("2026-09-26T10:00:07Z").unwrap(),
        slot: SlotIndex::new(3).unwrap(),
        context,
    });
    DeliveryRequest::new(
        &ActorId::parse(ACTOR).unwrap(),
        &data,
        TingMetadata::new(Some("deliberate".into())),
    )
    .unwrap()
}

fn deliver_request(d: &DeliveryRequest, key: &str) -> Request<Body> {
    authed("POST", "/api/v1/deliveries", ACCESS)
        .header("content-type", "application/json")
        .header("idempotency-key", key)
        .body(Body::from(d.to_bytes().unwrap()))
        .unwrap()
}

async fn ready(h: &Harness) {
    mount_silicon(&h.iam, ACCESS).await;
    mount_catalog(&h.iam).await;
    mount_exchange(&h.iam, None).await;
}

fn key_for(d: &DeliveryRequest) -> String {
    format!("peek-delivery-{}", d.event_id)
}

#[tokio::test]
async fn delivers_the_exact_deterministic_body() {
    let h = Harness::start().await;
    ready(&h).await;
    mount_ting_send(&h.ting, 202, false).await;
    let d = delivery(DataContext::Production);
    let r = h.send(deliver_request(&d, &key_for(&d))).await;
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(
        r.json(),
        json!({"event_id": d.event_id.to_string(), "ting_id": "msg_1", "status": "accepted", "silent": false, "replayed": false})
    );
    let sends = Harness::requests(&h.ting, "/v1/tings").await;
    assert_eq!(sends.len(), 1);
    let expected = ting_send_body(
        &OrgId::parse(ORG).unwrap(),
        &ActorId::parse(ACTOR).unwrap(),
        &d,
    )
    .unwrap();
    assert_eq!(
        sends[0].body, expected,
        "byte-identical to the deterministic body"
    );
    let text = String::from_utf8(sends[0].body.clone()).unwrap();
    assert!(text.starts_with(r#"{"org_id":"tos","type":"peek.ask.answered","for":"si:cleanup","key":"si:cleanup/ask_"#), "{text}");
    assert_eq!(sends[0].headers["authorization"], "Bearer proof-1");
    assert_eq!(sends[0].headers["content-type"], "application/json");
    assert!(
        !sends[0].headers.contains_key("idempotency-key"),
        "the body key is Ting's idempotency"
    );
    assert!(!sends[0].headers.contains_key("x-testing-environment-key"));

    let exchanges = Harness::requests(&h.iam, "/api/v1/obo-access/exchanges").await;
    assert_eq!(exchanges.len(), 1);
    let x: Value = serde_json::from_slice(&exchanges[0].body).unwrap();
    assert_eq!(x["endpoint_id"], "tings.send");
    assert_eq!(x["audience"], "ting");
    assert_eq!(x["org_id"], ORG);
    assert_eq!(x["metadata"], json!({}));
    assert_eq!(x["request"]["method"], "POST");
    assert_eq!(
        x["request"]["body_sha256"],
        silicon_iam_client::api::obo::body_sha256(&expected)
    );
    let send_event = h
        .events()
        .into_iter()
        .find(|e| e["event"] == "ting.send")
        .unwrap();
    assert_eq!(send_event["context"]["ting_status"], "accepted");
    assert_eq!(send_event["step"], "peek.ask.answered");
}

#[tokio::test]
async fn a_replay_with_the_same_key_never_resends() {
    let h = Harness::start().await;
    ready(&h).await;
    mount_ting_send(&h.ting, 202, true).await;
    let d = delivery(DataContext::Production);
    let first = h.send(deliver_request(&d, &key_for(&d))).await;
    assert_eq!(first.status, 200);
    assert_eq!(
        first.json()["silent"],
        true,
        "muted recipients are accepted silently"
    );
    let second = h.send(deliver_request(&d, &key_for(&d))).await;
    assert_eq!(second.status, 200);
    assert_eq!(second.body, first.body, "the stored response is replayed");
    assert_eq!(
        second.header("idempotent-replayed").as_deref(),
        Some("true")
    );
    assert_eq!(Harness::requests(&h.ting, "/v1/tings").await.len(), 1);

    // Same key, different body.
    let other = delivery(DataContext::Production);
    let r = h.send(deliver_request(&other, &key_for(&d))).await;
    assert_eq!(r.status, 409);
    assert_eq!(r.code(), "idempotency_conflict");

    // Same event under a new key: answered from the delivery record.
    let r = h
        .send(deliver_request(&d, "peek-delivery-another-key-01"))
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json()["replayed"], true);
    assert_eq!(Harness::requests(&h.ting, "/v1/tings").await.len(), 1);
}

#[tokio::test]
async fn ting_200_means_replayed() {
    let h = Harness::start().await;
    ready(&h).await;
    mount_ting_send(&h.ting, 200, false).await;
    let d = delivery(DataContext::Production);
    let r = h.send(deliver_request(&d, &key_for(&d))).await;
    assert_eq!(r.json()["replayed"], true);
}

#[tokio::test]
async fn proof_errors_are_retried_once_with_a_fresh_proof_and_the_same_bytes() {
    let h = Harness::start().await;
    ready(&h).await;
    Mock::given(method("POST"))
        .and(path("/v1/tings"))
        .respond_with(ting_error(401, "proof_consumed"))
        .up_to_n_times(1)
        .mount(&h.ting)
        .await;
    mount_ting_send(&h.ting, 202, false).await;
    let d = delivery(DataContext::Production);
    let r = h.send(deliver_request(&d, &key_for(&d))).await;
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let sends = Harness::requests(&h.ting, "/v1/tings").await;
    assert_eq!(sends.len(), 2);
    assert_eq!(sends[0].body, sends[1].body, "never re-serialized");
    assert_eq!(sends[0].headers["authorization"], "Bearer proof-1");
    assert_eq!(
        sends[1].headers["authorization"], "Bearer proof-2",
        "never reuse a proof"
    );
    let exchanges = Harness::requests(&h.iam, "/api/v1/obo-access/exchanges").await;
    assert_ne!(
        exchanges[0].headers["idempotency-key"], exchanges[1].headers["idempotency-key"],
        "a fresh exchange key per attempt"
    );
}

#[tokio::test]
async fn two_proof_errors_are_ting_unavailable() {
    let h = Harness::start().await;
    ready(&h).await;
    Mock::given(method("POST"))
        .and(path("/v1/tings"))
        .respond_with(ting_error(401, "invalid_proof"))
        .mount(&h.ting)
        .await;
    let d = delivery(DataContext::Production);
    let r = h.send(deliver_request(&d, &key_for(&d))).await;
    assert_eq!(r.status, 503);
    assert_eq!(r.code(), "ting_unavailable");
    assert_eq!(Harness::requests(&h.ting, "/v1/tings").await.len(), 2);
}

#[tokio::test]
async fn the_error_matrix() {
    let cases: [(ResponseTemplate, u16, &str); 7] = [
        (
            ting_error(403, "recipient_not_registered"),
            409,
            "recipient_not_registered",
        ),
        (ting_error(403, "permission_denied"), 502, "ting_rejected"),
        (
            ting_error(403, "test_context_mismatch"),
            502,
            "ting_rejected",
        ),
        (ting_error(404, "not_found"), 502, "ting_type_missing"),
        (
            ting_error(409, "idempotency_conflict"),
            409,
            "ting_key_conflict",
        ),
        (
            ting_error(429, "temporarily_rate_limited").insert_header("retry-after", "17"),
            503,
            "ting_unavailable",
        ),
        (
            ting_error(503, "proof_verification_uncertain"),
            503,
            "ting_unavailable",
        ),
    ];
    for (template, status, code) in cases {
        let h = Harness::start().await;
        ready(&h).await;
        Mock::given(method("POST"))
            .and(path("/v1/tings"))
            .respond_with(template)
            .mount(&h.ting)
            .await;
        let d = delivery(DataContext::Production);
        let r = h.send(deliver_request(&d, &key_for(&d))).await;
        assert_eq!(
            r.status,
            status,
            "{code}: {}",
            String::from_utf8_lossy(&r.body)
        );
        assert_eq!(r.code(), code);
        if code == "ting_unavailable" {
            assert_eq!(r.json()["error"]["retryable"], true);
            assert!(r.json()["error"]["details"]["retry_after"].is_u64());
            assert!(r.header("retry-after").is_some());
        }
        if status == 503 && r.header("retry-after").as_deref() == Some("17") {
            assert_eq!(r.json()["error"]["details"]["retry_after"], 17);
        }
        assert!(
            Harness::requests(&h.ting, "/v1/subscriptions")
                .await
                .is_empty(),
            "D7: never re-register"
        );
    }
}

#[tokio::test]
async fn recipient_not_registered_marks_the_enrollment_revoked() {
    let h = Harness::start().await;
    ready(&h).await;
    mount_ting_register(&h.ting).await;
    let enroll = h
        .send(json_body(
            authed("POST", "/api/v1/ting/recipient", ACCESS)
                .header("content-type", "application/json")
                .header("idempotency-key", IDEM),
            &json!({}),
        ))
        .await;
    assert_eq!(
        enroll.status,
        200,
        "{}",
        String::from_utf8_lossy(&enroll.body)
    );
    assert_eq!(
        enroll.json(),
        json!({"subscribed": true, "subscription_id": "sub_1"})
    );
    let me = h
        .send(
            authed("GET", "/api/v1/auth/me", ACCESS)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(me.json()["ting"]["subscribed"], true);

    Mock::given(method("POST"))
        .and(path("/v1/tings"))
        .respond_with(ting_error(403, "recipient_not_registered"))
        .mount(&h.ting)
        .await;
    let d = delivery(DataContext::Production);
    let r = h.send(deliver_request(&d, &key_for(&d))).await;
    assert_eq!(r.status, 409);
    let me = h
        .send(
            authed("GET", "/api/v1/auth/me", ACCESS)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(me.json()["ting"]["subscribed"], false);
    assert_eq!(
        Harness::requests(&h.ting, "/v1/subscriptions").await.len(),
        1,
        "no automatic re-registration"
    );
}

#[tokio::test]
async fn deliveries_are_validated_against_the_verified_identity() {
    let h = Harness::start().await;
    ready(&h).await;
    mount_ting_send(&h.ting, 202, false).await;

    // A key naming another actor.
    let mut d = delivery(DataContext::Production);
    d.key = d.key.replacen("si:cleanup", "si:intruder", 1);
    let r = h.send(deliver_request(&d, &key_for(&d))).await;
    assert_eq!(r.status, 400);
    assert_eq!(r.code(), "invalid_input");

    // A payload in the wrong data context.
    let d = delivery(DataContext::Testing);
    let r = h.send(deliver_request(&d, &key_for(&d))).await;
    assert_eq!(r.status, 400);

    // Unknown data fields and types are refused.
    let d = delivery(DataContext::Production);
    let mut v: Value = serde_json::from_slice(&d.to_bytes().unwrap()).unwrap();
    v["data"]["extra"] = json!(1);
    let r = h
        .send(json_body(
            authed("POST", "/api/v1/deliveries", ACCESS).header("idempotency-key", IDEM),
            &v,
        ))
        .await;
    assert_eq!(r.status, 400);
    v["data"].as_object_mut().unwrap().remove("extra");
    v["type"] = json!("peek.other.event");
    let r = h
        .send(json_body(
            authed("POST", "/api/v1/deliveries", ACCESS).header("idempotency-key", IDEM),
            &v,
        ))
        .await;
    assert_eq!(r.status, 400);
    assert!(Harness::requests(&h.ting, "/v1/tings").await.is_empty());
}

#[tokio::test]
async fn a_missing_scope_is_reconsent_before_any_proof() {
    let h = Harness::start().await;
    mount_introspect(
        &h.iam,
        ACCESS,
        introspection(
            ACTOR,
            ORG,
            &["self.identity.read", "self.profile.read"],
            None,
            None,
        ),
    )
    .await;
    let d = delivery(DataContext::Production);
    let r = h.send(deliver_request(&d, &key_for(&d))).await;
    assert_eq!(r.status, 403);
    assert_eq!(r.code(), "reconsent_required");
    assert_eq!(
        r.json()["error"]["details"]["missing_scopes"],
        json!(["obo:ting:tings.send"])
    );
    assert!(
        Harness::requests(&h.iam, "/api/v1/obo-access/exchanges")
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn an_inactive_bearer_is_401_so_peekd_refreshes() {
    let h = Harness::start().await;
    mount_introspect(&h.iam, ACCESS, json!({"active": false})).await;
    let d = delivery(DataContext::Production);
    let r = h.send(deliver_request(&d, &key_for(&d))).await;
    assert_eq!(r.status, 401);
    assert_eq!(r.code(), "unauthenticated");
}

#[tokio::test]
async fn an_unsolicited_testing_context_in_production_is_refused() {
    let h = Harness::start().await;
    mount_silicon(&h.iam, ACCESS).await;
    mount_catalog(&h.iam).await;
    mount_exchange(
        &h.iam,
        Some(json!({"app_id": "ting", "app_secret": TING_TEST_SECRET, "iam_test_key": ROOT_KEY})),
    )
    .await;
    mount_ting_send(&h.ting, 202, false).await;
    let d = delivery(DataContext::Production);
    let r = h.send(deliver_request(&d, &key_for(&d))).await;
    assert_eq!(r.status, 502);
    assert_eq!(r.code(), "ting_rejected");
    assert!(
        Harness::requests(&h.ting, "/v1/tings").await.is_empty(),
        "never sent"
    );
}
