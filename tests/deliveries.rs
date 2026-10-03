//! `POST /api/v1/deliveries` and `POST /api/v1/ting/recipient` against fake
//! IAM and Ting: the deterministic body, one fresh proof per attempt, the
//! retry on proof errors, the §3.6 error matrix, idempotency and D7; and the
//! peek 0.1.2 allowlist (`peek.send.expired`, `peek.schedule.due`,
//! `peek.send.shown`, the additive `shown` on `peek.ask.expired`, the
//! `esc_double` gesture) with their strict schemas, keys and plane checks.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use axum::{body::Body, http::Request};
use common::*;
use serde_json::{Value, json};
use silicon_peek_client::{
    identity::{ActorId, DataContext, OrgId, SlotIndex},
    ids::{AskId, EventId, ScheduleId, SendId},
    schema::ask::{Answer, AskType},
    timestamp::Timestamp,
    ting::{
        AnswerVia, AskAnswered, AskExpired, DeliveryRequest, DueOutcome, ScheduleDue, SchemaV1,
        SendExpired, SendShown, TingData, TingMetadata, TingType, WaitingReason, ting_send_body,
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
    assert_eq!(h.approve_ting(ACCESS, false).await.status, 200);
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
        serde_json::from_slice::<Value>(&sends[0].body).unwrap(),
        serde_json::from_slice::<Value>(&expected).unwrap()
    );
    assert_eq!(
        sends[0].headers["authorization"],
        "Bearer oba_fixture_tings.send"
    );
    assert_eq!(sends[0].headers["content-type"], "application/json");
    assert!(
        !sends[0].headers.contains_key("idempotency-key"),
        "the body key is Ting's idempotency"
    );
    assert!(!sends[0].headers.contains_key("x-testing-environment-key"));

    assert!(
        Harness::requests(&h.iam, "/api/v1/obo-access/exchanges")
            .await
            .is_empty(),
        "the retired flow is never called"
    );
    assert_eq!(
        Harness::requests(&h.iam, "/api/v1/obo-access/tokens")
            .await
            .len(),
        1,
        "the root is reused after approval"
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
async fn token_errors_refresh_once_and_keep_the_same_operation_bytes() {
    let h = Harness::start().await;
    ready(&h).await;
    Mock::given(method("POST"))
        .and(path("/v1/tings"))
        .respond_with(ting_error(401, "invalid_obo_token"))
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
    assert_eq!(
        sends[0].headers["authorization"],
        "Bearer oba_fixture_tings.send"
    );
    assert_eq!(
        sends[1].headers["authorization"], "Bearer oba_rotated_tings.send",
        "never reuse a proof"
    );
    let exchanges = Harness::requests(&h.iam, "/api/v1/obo-access/tokens").await;
    assert_eq!(exchanges.len(), 2, "code redemption and one root refresh");
    let refresh: Value = serde_json::from_slice(&exchanges[1].body).unwrap();
    assert_eq!(refresh["refresh_token"], "obr_fixture_tings.send");
}

#[tokio::test]
async fn repeated_token_rejection_requires_feature_permission_without_logging_out() {
    let h = Harness::start().await;
    ready(&h).await;
    Mock::given(method("POST"))
        .and(path("/v1/tings"))
        .respond_with(ting_error(401, "invalid_proof"))
        .mount(&h.ting)
        .await;
    let d = delivery(DataContext::Production);
    let r = h.send(deliver_request(&d, &key_for(&d))).await;
    assert_eq!(r.status, 403);
    assert_eq!(r.code(), "reconsent_required");
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
    assert_eq!(r.json()["error"]["details"]["feature"], json!("ting"));
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
    let r = h.approve_ting(ACCESS, false).await;
    assert_eq!(r.status, 502);
    assert_eq!(r.code(), "iam_unavailable");
    assert!(
        Harness::requests(&h.ting, "/v1/tings").await.is_empty(),
        "never sent"
    );
}

// ---------------------------------------------------------------------------
// peek 0.1.2 Ting types (contract §3.10, §7, §12.4). The fixtures are raw JSON
// in the contract's wire shape, so these tests pin what peek-server accepts
// independently of how peekd builds it; `peekd_built_deliveries_are_accepted`
// covers the typed path.

/// The IDs one fixture set refers to.
struct Ids {
    send: SendId,
    ask: AskId,
    schedule: ScheduleId,
    other_send: SendId,
}

impl Ids {
    fn new() -> Self {
        Self {
            send: SendId::generate(),
            ask: AskId::generate(),
            schedule: ScheduleId::generate(),
            other_send: SendId::generate(),
        }
    }
}

/// One delivery under test: the type name, the Ting key and `data`.
#[derive(Clone)]
struct Case {
    ting_type: &'static str,
    key: String,
    data: Value,
}

impl Case {
    /// The outbox JSON peekd posts, with a fresh event ID.
    fn body(&self) -> Value {
        json!({
            "event_id": EventId::generate(),
            "type": self.ting_type,
            "key": self.key,
            "data": self.data,
            "metadata": {"isi": "deliberate", "peek_version": "0.1.2"}
        })
    }

    /// A variant is a distinct recorded action, not a changed retry of the
    /// same Ting operation. Give its keyed subject a fresh identity.
    fn independent(mut self) -> Self {
        let (field, fresh) = if self.ting_type == "peek.schedule.due" {
            ("schedule_id", ScheduleId::generate().to_string())
        } else if self.ting_type.starts_with("peek.ask.") {
            ("ask_id", AskId::generate().to_string())
        } else {
            ("send_id", SendId::generate().to_string())
        };
        let old = self.data[field].as_str().unwrap().to_owned();
        self.key = self.key.replace(&old, &fresh);
        self.data[field] = json!(fresh);
        self
    }

    /// A copy with `data[field]` set to `value`.
    fn with(&self, field: &str, value: Value) -> Self {
        let mut c = self.clone();
        c.data
            .as_object_mut()
            .unwrap()
            .insert(field.to_owned(), value);
        c
    }

    /// A copy without `data[field]`.
    fn without(&self, field: &str) -> Self {
        let mut c = self.clone();
        c.data.as_object_mut().unwrap().remove(field);
        c
    }

    /// A copy with another Ting key.
    fn keyed(&self, key: String) -> Self {
        let mut c = self.clone();
        c.key = key;
        c
    }

    /// A copy delivered under another type name.
    fn typed(&self, ting_type: &'static str) -> Self {
        let mut c = self.clone();
        c.ting_type = ting_type;
        c
    }

    /// The exact `/v1/tings` body peek-server must build (field order
    /// `org_id, type, for, key, data, metadata`; `data` forwarded as parsed).
    fn expected_ting_body(&self) -> String {
        format!(
            r#"{{"org_id":"{ORG}","type":"{}","for":"{ACTOR}","key":"{}","data":{},"metadata":{{"isi":"deliberate","peek_version":"0.1.2"}}}}"#,
            self.ting_type, self.key, self.data
        )
    }
}

/// `peek.send.expired`: a waiting show that was never on screen.
fn send_expired(ids: &Ids, context: &str) -> Case {
    Case {
        ting_type: "peek.send.expired",
        key: format!("{ACTOR}/{}/send_expired", ids.send),
        data: json!({
            "schema": 1, "send_id": ids.send, "kind": "show",
            "created_at": "2026-09-27T12:00:00.000Z",
            "expires_at": "2026-09-27T12:10:00.000Z",
            "expired_at": "2026-09-27T12:10:00.004Z",
            "shown": false, "shown_at": null,
            "scheduled": false, "schedule_id": null,
            "slot": 3, "context": context
        }),
    }
}

/// `peek.schedule.due`: the contract's example, a scheduled show that came due
/// behind another send.
fn schedule_due(ids: &Ids, context: &str) -> Case {
    Case {
        ting_type: "peek.schedule.due",
        key: format!("{ACTOR}/{}/due", ids.schedule),
        data: json!({
            "schema": 1, "schedule_id": ids.schedule, "send_id": ids.send, "ask_id": null,
            "kind": "show",
            "due_at": "2026-09-27T12:30:00.000Z", "fired_at": "2026-09-27T12:30:00.012Z",
            "outcome": "queued", "replaced_send_id": null,
            "waiting_reason": "behind_others", "queue_position": 2,
            "expires_at": null, "slot": 3, "context": context
        }),
    }
}

/// A valid production `peek.schedule.due` for `outcome`.
fn due(ids: &Ids, outcome: &str) -> Case {
    let queued = schedule_due(ids, "production");
    match outcome {
        "shown" => queued
            .with("outcome", json!("shown"))
            .with("waiting_reason", Value::Null)
            .with("queue_position", Value::Null),
        "expired" => queued
            .with("outcome", json!("expired"))
            .with("waiting_reason", Value::Null)
            .with("queue_position", Value::Null)
            .with("expires_at", json!("2026-09-27T12:40:00.000Z"))
            // Fired on catch-up after the Mac woke up.
            .with("fired_at", json!("2026-09-28T07:15:02.500Z")),
        "replaced" => queued
            .with("outcome", json!("replaced"))
            .with("replaced_send_id", json!(ids.other_send))
            .with("waiting_reason", Value::Null)
            .with("queue_position", Value::Null),
        _ => queued,
    }
}

/// `peek.send.shown`: a scheduled speak+show that appeared.
fn send_shown(ids: &Ids, context: &str) -> Case {
    Case {
        ting_type: "peek.send.shown",
        key: format!("{ACTOR}/{}/shown", ids.send),
        data: json!({
            "schema": 1, "send_id": ids.send, "ask_id": null, "kind": "speak+show",
            "created_at": "2026-09-27T12:00:00.000Z", "shown_at": "2026-09-27T12:30:00.250Z",
            "scheduled": true, "schedule_id": ids.schedule,
            "slot": 3, "context": context
        }),
    }
}

/// `peek.ask.expired` exactly as peekd 0.1.1 sends it (no `shown`).
fn ask_expired(ids: &Ids, context: &str) -> Case {
    Case {
        ting_type: "peek.ask.expired",
        key: format!("{ACTOR}/{}/expired", ids.ask),
        data: json!({
            "schema": 1, "ask_id": ids.ask, "send_id": ids.send,
            "question": "Delete old.zip?", "ask_type": "text",
            "asked_at": "2026-09-27T12:00:00.000Z", "expired_at": "2026-09-27T12:10:00.000Z",
            "slot": 3, "context": context
        }),
    }
}

fn show_dismissed(ids: &Ids, gesture: &str) -> Case {
    Case {
        ting_type: "peek.show.dismissed",
        key: format!("{ACTOR}/{}/show_dismissed", ids.send),
        data: json!({
            "schema": 1, "send_id": ids.send, "gesture": gesture, "visible_ms": 900,
            "dismissed_at": "2026-09-27T12:00:01.000Z", "slot": 3, "context": "production"
        }),
    }
}

fn ask_dismissed(ids: &Ids, gesture: &str) -> Case {
    Case {
        ting_type: "peek.ask.dismissed",
        key: format!("{ACTOR}/{}/dismissed", ids.ask),
        data: json!({
            "schema": 1, "ask_id": ids.ask, "send_id": ids.send,
            "question": "Delete old.zip?", "ask_type": "text", "gesture": gesture,
            "asked_at": "2026-09-27T12:00:00.000Z", "dismissed_at": "2026-09-27T12:00:02.000Z",
            "slot": 3, "context": "production"
        }),
    }
}

/// The three types 0.1.2 adds, in registration order.
fn new_types(ids: &Ids, context: &str) -> [Case; 3] {
    [
        send_expired(ids, context),
        schedule_due(ids, context),
        send_shown(ids, context),
    ]
}

/// A delivery request for raw outbox JSON; `testing_plane` adds the testing
/// headers (generation 1).
fn deliver_raw(body: &Value, token: &str, testing_plane: bool) -> Request<Body> {
    let event_id = body["event_id"].as_str().unwrap();
    let mut b = authed("POST", "/api/v1/deliveries", token)
        .header("content-type", "application/json")
        .header("idempotency-key", format!("peek-delivery-{event_id}"));
    if testing_plane {
        b = testing(b, Some(1));
    }
    b.body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

/// Delivers `case` in production and expects Ting's acceptance.
async fn accept(h: &Harness, label: &str, case: &Case) {
    let body = case.body();
    let r = h.send(deliver_raw(&body, ACCESS, false)).await;
    assert_eq!(
        r.status,
        200,
        "{label}: {}",
        String::from_utf8_lossy(&r.body)
    );
    assert_eq!(r.json()["event_id"], body["event_id"], "{label}");
    assert_eq!(r.json()["status"], "accepted", "{label}");
}

/// Delivers `case` in production and expects `400 invalid_input`; returns the
/// error message.
async fn refuse(h: &Harness, label: &str, case: &Case) -> String {
    let r = h.send(deliver_raw(&case.body(), ACCESS, false)).await;
    assert_eq!(
        r.status,
        400,
        "{label} must be refused: {}",
        String::from_utf8_lossy(&r.body)
    );
    assert_eq!(r.code(), "invalid_input", "{label}");
    r.json()["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

#[tokio::test]
async fn the_new_types_deliver_the_exact_deterministic_body() {
    let h = Harness::start().await;
    ready(&h).await;
    mount_ting_send(&h.ting, 202, false).await;
    let ids = Ids::new();
    let org = OrgId::parse(ORG).unwrap();
    let actor = ActorId::parse(ACTOR).unwrap();
    for (i, case) in new_types(&ids, "production").iter().enumerate() {
        let body = case.body();
        let r = h.send(deliver_raw(&body, ACCESS, false)).await;
        assert_eq!(
            r.status,
            200,
            "{}: {}",
            case.ting_type,
            String::from_utf8_lossy(&r.body)
        );
        assert_eq!(
            r.json(),
            json!({"event_id": body["event_id"], "ting_id": "msg_1", "status": "accepted", "silent": false, "replayed": false})
        );
        let sends = Harness::requests(&h.ting, "/v1/tings").await;
        assert_eq!(sends.len(), i + 1);
        assert_eq!(
            String::from_utf8(sends[i].body.clone()).unwrap(),
            case.expected_ting_body(),
            "{}",
            case.ting_type
        );
        let parsed: DeliveryRequest = serde_json::from_value(body).unwrap();
        assert_eq!(
            sends[i].body,
            ting_send_body(&org, &actor, &parsed).unwrap(),
            "{}: the deterministic body",
            case.ting_type
        );
        assert_eq!(
            sends[i].headers["authorization"],
            "Bearer oba_fixture_tings.send"
        );
    }
    let steps: Vec<Value> = h
        .events()
        .into_iter()
        .filter(|e| e["event"] == "ting.send")
        .map(|e| e["step"].clone())
        .collect();
    assert_eq!(
        steps,
        [
            json!("peek.send.expired"),
            json!("peek.schedule.due"),
            json!("peek.send.shown")
        ]
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // a data table: one row per documented variant
async fn the_new_types_accept_every_documented_variant() {
    let h = Harness::start().await;
    ready(&h).await;
    mount_ting_send(&h.ting, 202, false).await;
    let ids = Ids::new();
    let expired = send_expired(&ids, "production");
    let queued = schedule_due(&ids, "production");
    let shown = send_shown(&ids, "production");
    let cases = [
        ("send.expired: waiting show, never shown", expired.clone()),
        (
            "send.expired: scheduled speak that was on screen",
            expired
                .with("kind", json!("speak"))
                .with("shown", json!(true))
                .with("shown_at", json!("2026-09-27T12:00:01.200Z"))
                .with("scheduled", json!(true))
                .with("schedule_id", json!(ids.schedule)),
        ),
        (
            "send.expired: speak+show on slot 8",
            expired
                .with("kind", json!("speak+show"))
                .with("slot", json!(8)),
        ),
        (
            "send.expired: expired the instant it was created",
            expired.with("expired_at", json!("2026-09-27T12:00:00.000Z")),
        ),
        ("schedule.due: shown", due(&ids, "shown")),
        ("schedule.due: queued behind others", queued.clone()),
        (
            "schedule.due: queued as due-waiting overflow",
            queued
                .with("waiting_reason", json!("queue_full"))
                .with("queue_position", json!(6)),
        ),
        (
            "schedule.due: held while the Carbon is away",
            queued
                .with("waiting_reason", json!("carbon_away"))
                .with("queue_position", json!(0)),
        ),
        (
            "schedule.due: held while paused",
            queued
                .with("waiting_reason", json!("paused"))
                .with("queue_position", json!(0)),
        ),
        (
            "schedule.due: held while Peek.app is not running",
            queued
                .with("waiting_reason", json!("app_not_running"))
                .with("queue_position", json!(0)),
        ),
        ("schedule.due: expired on catch-up", due(&ids, "expired")),
        (
            "schedule.due: replaced the active peek",
            due(&ids, "replaced"),
        ),
        (
            "schedule.due: replaced but held",
            due(&ids, "replaced").with("waiting_reason", json!("carbon_away")),
        ),
        (
            "schedule.due: a speak+ask fired on time",
            due(&ids, "shown")
                .with("kind", json!("speak+ask"))
                .with("ask_id", json!(ids.ask))
                .with("fired_at", json!("2026-09-27T12:30:00.000Z"))
                .with("expires_at", json!("2026-09-27T13:00:00.000Z")),
        ),
        ("send.shown: scheduled", shown.clone()),
        (
            "send.shown: --notify shown on an ask",
            shown
                .with("kind", json!("ask"))
                .with("ask_id", json!(ids.ask))
                .with("scheduled", json!(false))
                .with("schedule_id", Value::Null),
        ),
        (
            "send.shown: speak shown the instant it was created",
            shown
                .with("kind", json!("speak"))
                .with("shown_at", json!("2026-09-27T12:00:00.000Z")),
        ),
    ];
    let cases = cases.map(|(label, case)| (label, case.independent()));
    for (label, case) in &cases {
        accept(&h, label, case).await;
    }
    let sends = Harness::requests(&h.ting, "/v1/tings").await;
    assert_eq!(sends.len(), cases.len());
    for ((label, case), sent) in cases.iter().zip(&sends) {
        assert_eq!(
            String::from_utf8(sent.body.clone()).unwrap(),
            case.expected_ting_body(),
            "{label}"
        );
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // a data table: one row per refused shape
async fn the_new_types_are_strict() {
    let h = Harness::start().await;
    ready(&h).await;
    mount_ting_send(&h.ting, 202, false).await;
    let ids = Ids::new();
    let se = send_expired(&ids, "production");
    let sd = schedule_due(&ids, "production");
    let ss = send_shown(&ids, "production");
    // (label, delivery, a fragment the refusal must name: the field or rule),
    // so no case passes for an unrelated reason.
    let cases = [
        // Parsing: unknown fields, schema, slot, context, types, missing fields.
        (
            "send.expired: unknown field",
            se.with("reason", json!("timeout")),
            "`reason`",
        ),
        (
            "send.expired: schema 2",
            se.with("schema", json!(2)),
            "schema 2",
        ),
        (
            "send.expired: slot 9",
            se.with("slot", json!(9)),
            "position 9",
        ),
        (
            "send.expired: slot 0",
            se.with("slot", json!(0)),
            "position 0",
        ),
        (
            "send.expired: unknown context",
            se.with("context", json!("staging")),
            "`staging`",
        ),
        (
            "send.expired: missing expires_at",
            se.without("expires_at"),
            "`expires_at`",
        ),
        (
            "send.expired: missing expired_at",
            se.without("expired_at"),
            "`expired_at`",
        ),
        (
            "send.expired: missing shown",
            se.without("shown"),
            "`shown`",
        ),
        (
            "send.expired: missing scheduled",
            se.without("scheduled"),
            "`scheduled`",
        ),
        (
            "send.expired: shown is not a bool",
            se.with("shown", json!("no")),
            "boolean",
        ),
        (
            "send.expired: expired_at is not RFC 3339",
            se.with("expired_at", json!("yesterday")),
            "`yesterday`",
        ),
        (
            "send.expired: send_id is an ask ID",
            se.with("send_id", json!(ids.ask))
                .keyed(format!("{ACTOR}/{}/send_expired", ids.ask)),
            "`snd_`",
        ),
        (
            "send.expired: schedule_id is a send ID",
            se.with("scheduled", json!(true))
                .with("schedule_id", json!(ids.other_send)),
            "`sch_`",
        ),
        // Semantic rules.
        (
            "send.expired: kind ask",
            se.with("kind", json!("ask")),
            "kind `ask`",
        ),
        (
            "send.expired: kind speak+ask",
            se.with("kind", json!("speak+ask")),
            "kind `speak+ask`",
        ),
        (
            "send.expired: kind empty",
            se.with("kind", json!("")),
            "kind ``",
        ),
        (
            "send.expired: kind in capitals",
            se.with("kind", json!("SHOW")),
            "kind `SHOW`",
        ),
        (
            "send.expired: shown without shown_at",
            se.with("shown", json!(true)),
            "shown_at",
        ),
        (
            "send.expired: shown_at while never shown",
            se.with("shown_at", json!("2026-09-27T12:05:00.000Z")),
            "shown_at",
        ),
        (
            "send.expired: scheduled without schedule_id",
            se.with("scheduled", json!(true)),
            "schedule_id",
        ),
        (
            "send.expired: schedule_id while not scheduled",
            se.with("schedule_id", json!(ids.schedule)),
            "schedule_id",
        ),
        (
            "send.expired: expired before it was created",
            se.with("expired_at", json!("2026-09-27T11:59:59.999Z")),
            "expired_at",
        ),
        // peek.schedule.due parsing.
        (
            "schedule.due: unknown field",
            sd.with("recurrence", json!("daily")),
            "`recurrence`",
        ),
        (
            "schedule.due: schema 2",
            sd.with("schema", json!(2)),
            "schema 2",
        ),
        (
            "schedule.due: slot 9",
            sd.with("slot", json!(9)),
            "position 9",
        ),
        (
            "schedule.due: unknown context",
            sd.with("context", json!("staging")),
            "`staging`",
        ),
        (
            "schedule.due: unknown outcome",
            sd.with("outcome", json!("delivered")),
            "`delivered`",
        ),
        (
            "schedule.due: unknown waiting_reason",
            sd.with("waiting_reason", json!("asleep")),
            "`asleep`",
        ),
        (
            "schedule.due: negative queue_position",
            sd.with("queue_position", json!(-1)),
            "`-1`",
        ),
        (
            "schedule.due: missing due_at",
            sd.without("due_at"),
            "`due_at`",
        ),
        (
            "schedule.due: missing outcome",
            sd.without("outcome"),
            "`outcome`",
        ),
        (
            "schedule.due: schedule_id is a send ID",
            sd.with("schedule_id", json!(ids.other_send))
                .keyed(format!("{ACTOR}/{}/due", ids.other_send)),
            "`sch_`",
        ),
        (
            "schedule.due: replaced_send_id is a schedule ID",
            due(&ids, "replaced").with("replaced_send_id", json!(ids.schedule)),
            "`snd_`",
        ),
        // peek.schedule.due semantic rules.
        (
            "schedule.due: unknown kind",
            sd.with("kind", json!("video")),
            "kind `video`",
        ),
        (
            "schedule.due: queued without queue_position",
            sd.with("queue_position", Value::Null),
            "queue_position",
        ),
        (
            "schedule.due: queued without waiting_reason",
            sd.with("waiting_reason", Value::Null),
            "waiting_reason",
        ),
        (
            "schedule.due: queued with replaced_send_id",
            sd.with("replaced_send_id", json!(ids.other_send)),
            "replaced_send_id",
        ),
        (
            "schedule.due: shown with queue_position",
            due(&ids, "shown").with("queue_position", json!(0)),
            "queue_position",
        ),
        (
            "schedule.due: shown with waiting_reason",
            due(&ids, "shown").with("waiting_reason", json!("behind_others")),
            "waiting_reason",
        ),
        (
            "schedule.due: shown with replaced_send_id",
            due(&ids, "shown").with("replaced_send_id", json!(ids.other_send)),
            "replaced_send_id",
        ),
        (
            "schedule.due: expired with waiting_reason",
            due(&ids, "expired").with("waiting_reason", json!("carbon_away")),
            "waiting_reason",
        ),
        (
            "schedule.due: expired with queue_position",
            due(&ids, "expired").with("queue_position", json!(1)),
            "queue_position",
        ),
        (
            "schedule.due: replaced without replaced_send_id",
            due(&ids, "replaced").with("replaced_send_id", Value::Null),
            "replaced_send_id",
        ),
        (
            "schedule.due: replaced with queue_position",
            due(&ids, "replaced").with("queue_position", json!(1)),
            "queue_position",
        ),
        (
            "schedule.due: fired before it was due",
            sd.with("fired_at", json!("2026-09-27T12:29:59.999Z")),
            "fired_at",
        ),
        // peek.send.shown.
        (
            "send.shown: unknown field",
            ss.with("visible_ms", json!(10)),
            "`visible_ms`",
        ),
        (
            "send.shown: schema 2",
            ss.with("schema", json!(2)),
            "schema 2",
        ),
        (
            "send.shown: slot 9",
            ss.with("slot", json!(9)),
            "position 9",
        ),
        (
            "send.shown: unknown context",
            ss.with("context", json!("staging")),
            "`staging`",
        ),
        (
            "send.shown: missing shown_at",
            ss.without("shown_at"),
            "`shown_at`",
        ),
        (
            "send.shown: missing created_at",
            ss.without("created_at"),
            "`created_at`",
        ),
        (
            "send.shown: scheduled is not a bool",
            ss.with("scheduled", json!("yes")),
            "boolean",
        ),
        (
            "send.shown: unknown kind",
            ss.with("kind", json!("image")),
            "kind `image`",
        ),
        (
            "send.shown: scheduled without schedule_id",
            ss.with("schedule_id", Value::Null),
            "schedule_id",
        ),
        (
            "send.shown: schedule_id while not scheduled",
            ss.with("scheduled", json!(false)),
            "schedule_id",
        ),
        (
            "send.shown: shown before it was created",
            ss.with("shown_at", json!("2026-09-27T11:59:59.000Z")),
            "shown_at",
        ),
        (
            "send.shown: ask_id is a send ID",
            ss.with("kind", json!("ask"))
                .with("ask_id", json!(ids.other_send)),
            "`ask_`",
        ),
    ];
    for (label, case, fragment) in &cases {
        let message = refuse(&h, label, case).await;
        assert!(
            message.contains(fragment),
            "{label}: the refusal names {fragment}: {message}"
        );
    }
    assert!(
        Harness::requests(&h.ting, "/v1/tings").await.is_empty(),
        "nothing invalid reaches Ting"
    );
    assert!(
        Harness::requests(&h.iam, "/api/v1/obo-access/exchanges")
            .await
            .is_empty(),
        "no proof is minted for an invalid delivery"
    );
}

#[tokio::test]
async fn new_type_keys_name_the_verified_actor_the_subject_and_the_event() {
    let h = Harness::start().await;
    ready(&h).await;
    mount_ting_send(&h.ting, 202, false).await;
    let ids = Ids::new();
    let [se, sd, ss] = new_types(&ids, "production");
    let cases = [
        (
            "send.expired: another actor",
            se.keyed(se.key.replacen(ACTOR, "si:intruder", 1)),
            &se.key,
        ),
        (
            "send.expired: another send",
            se.keyed(format!("{ACTOR}/{}/send_expired", ids.other_send)),
            &se.key,
        ),
        (
            "send.expired: the ask event name",
            se.keyed(format!("{ACTOR}/{}/expired", ids.send)),
            &se.key,
        ),
        (
            "send.expired: keyed by its schedule",
            se.with("scheduled", json!(true))
                .with("schedule_id", json!(ids.schedule))
                .keyed(format!("{ACTOR}/{}/send_expired", ids.schedule)),
            &se.key,
        ),
        (
            "schedule.due: another actor",
            sd.keyed(sd.key.replacen(ACTOR, "si:intruder", 1)),
            &sd.key,
        ),
        (
            "schedule.due: keyed by its send",
            sd.keyed(format!("{ACTOR}/{}/due", ids.send)),
            &sd.key,
        ),
        (
            "schedule.due: wrong event name",
            sd.keyed(format!("{ACTOR}/{}/scheduled", ids.schedule)),
            &sd.key,
        ),
        (
            "send.shown: another actor",
            ss.keyed(ss.key.replacen(ACTOR, "si:intruder", 1)),
            &ss.key,
        ),
        (
            "send.shown: keyed by its schedule",
            ss.keyed(format!("{ACTOR}/{}/shown", ids.schedule)),
            &ss.key,
        ),
        (
            "send.shown: wrong event name",
            ss.keyed(format!("{ACTOR}/{}/send_shown", ids.send)),
            &ss.key,
        ),
    ];
    for (label, case, expected) in &cases {
        let message = refuse(&h, label, case).await;
        assert!(
            message.contains(expected.as_str()),
            "{label}: the error names the expected key {expected}: {message}"
        );
    }
    assert!(Harness::requests(&h.ting, "/v1/tings").await.is_empty());
}

#[tokio::test]
async fn new_types_in_the_wrong_plane_are_refused() {
    let h = Harness::start().await;
    ready(&h).await;
    mount_ting_send(&h.ting, 202, false).await;
    let ids = Ids::new();
    for case in new_types(&ids, "testing") {
        let message = refuse(&h, case.ting_type, &case).await;
        assert!(
            message
                .contains("data.context is `testing` but this request is in the production plane"),
            "{}: {message}",
            case.ting_type
        );
    }
    assert!(Harness::requests(&h.ting, "/v1/tings").await.is_empty());
}

#[tokio::test]
async fn new_types_deliver_in_a_testing_plane_with_testing_data_only() {
    const TEST_ACCESS: &str = "oat_testPlaneAccess01";
    let h = Harness::start().await;
    mount_testing_contexts(&h.iam).await;
    mount_introspect_testing(
        &h.iam,
        TEST_ACCESS,
        introspection(ACTOR, ORG, &FULL_SCOPES, Some("admin"), Some(env_id())),
    )
    .await;
    h.prepare_environment().await;
    mount_catalog(&h.iam).await;
    mount_exchange_for(
        &h.iam,
        TEST_SECRET,
        Some(json!({"app_id": "ting", "app_secret": TING_TEST_SECRET, "iam_test_key": ROOT_KEY})),
    )
    .await;
    let approval = h.approve_ting(TEST_ACCESS, true).await;
    assert_eq!(
        approval.status,
        200,
        "{}",
        String::from_utf8_lossy(&approval.body)
    );
    mount_ting_send(&h.ting, 202, false).await;
    let ids = Ids::new();

    for case in new_types(&ids, "production") {
        let r = h.send(deliver_raw(&case.body(), TEST_ACCESS, true)).await;
        assert_eq!(
            r.status,
            400,
            "{}: {}",
            case.ting_type,
            String::from_utf8_lossy(&r.body)
        );
        assert_eq!(r.code(), "invalid_input");
        let message = r.json()["error"]["message"].as_str().unwrap().to_owned();
        assert!(
            message
                .contains("data.context is `production` but this request is in the testing plane"),
            "{}: {message}",
            case.ting_type
        );
    }
    assert!(Harness::requests(&h.ting, "/v1/tings").await.is_empty());

    let cases = new_types(&ids, "testing");
    for case in &cases {
        let r = h.send(deliver_raw(&case.body(), TEST_ACCESS, true)).await;
        assert_eq!(
            r.status,
            200,
            "{}: {}",
            case.ting_type,
            String::from_utf8_lossy(&r.body)
        );
    }
    let sends = Harness::requests(&h.ting, "/v1/tings").await;
    assert_eq!(sends.len(), cases.len());
    for (case, sent) in cases.iter().zip(&sends) {
        assert_eq!(
            String::from_utf8(sent.body.clone()).unwrap(),
            case.expected_ting_body(),
            "{}",
            case.ting_type
        );
        assert_eq!(
            sent.headers["x-testing-environment-key"], ROOT_KEY,
            "{}: Ting's test headers come from the proof",
            case.ting_type
        );
    }
}

#[tokio::test]
async fn ask_expired_accepts_the_0_1_1_shape_and_the_additive_shown() {
    let h = Harness::start().await;
    ready(&h).await;
    mount_ting_send(&h.ting, 202, false).await;
    let ids = Ids::new();
    let legacy = ask_expired(&ids, "production");
    let accepted = [
        ("ask.expired: peekd 0.1.1 (no shown)", legacy.clone()),
        ("ask.expired: shown", legacy.with("shown", json!(true))),
        (
            "ask.expired: never shown (a waiting or scheduled ask)",
            legacy.with("shown", json!(false)),
        ),
    ];
    let accepted = accepted.map(|(label, case)| (label, case.independent()));
    for (label, case) in &accepted {
        accept(&h, label, case).await;
    }
    let sends = Harness::requests(&h.ting, "/v1/tings").await;
    assert_eq!(sends.len(), accepted.len());
    for ((label, case), sent) in accepted.iter().zip(&sends) {
        let text = String::from_utf8(sent.body.clone()).unwrap();
        assert_eq!(text, case.expected_ting_body(), "{label}");
    }
    let bodies: Vec<Value> = sends
        .iter()
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect();
    assert!(
        bodies[0]["data"].get("shown").is_none(),
        "a 0.1.1 body is forwarded without inventing `shown`"
    );
    assert_eq!(bodies[1]["data"]["shown"], true);
    assert_eq!(bodies[2]["data"]["shown"], false);

    for (label, case, fragment) in [
        (
            "ask.expired: shown is a string",
            legacy.with("shown", json!("yes")),
            "boolean",
        ),
        (
            "ask.expired: shown is a number",
            legacy.with("shown", json!(1)),
            "boolean",
        ),
        (
            "ask.expired: shown_at belongs to peek.send.expired",
            legacy.with("shown_at", json!("2026-09-27T12:00:00.000Z")),
            "`shown_at`",
        ),
        (
            "ask.expired: scheduled belongs to peek.send.expired",
            legacy.with("scheduled", json!(false)),
            "`scheduled`",
        ),
    ] {
        let message = refuse(&h, label, &case).await;
        assert!(message.contains(fragment), "{label}: {message}");
    }
    assert_eq!(Harness::requests(&h.ting, "/v1/tings").await.len(), 3);
}

#[tokio::test]
async fn esc_double_is_a_dismiss_gesture_for_shows_and_asks() {
    let h = Harness::start().await;
    ready(&h).await;
    mount_ting_send(&h.ting, 202, false).await;
    let ids = Ids::new();
    let accepted = [
        (
            "show.dismissed: esc_double",
            show_dismissed(&ids, "esc_double"),
        ),
        (
            "ask.dismissed: esc_double",
            ask_dismissed(&ids, "esc_double"),
        ),
        ("ask.dismissed: esc", ask_dismissed(&ids, "esc")),
        (
            "ask.dismissed: down_arrow",
            ask_dismissed(&ids, "down_arrow"),
        ),
    ];
    let accepted = accepted.map(|(label, case)| (label, case.independent()));
    for (label, case) in &accepted {
        accept(&h, label, case).await;
    }
    let sends = Harness::requests(&h.ting, "/v1/tings").await;
    for ((label, case), sent) in accepted.iter().zip(&sends) {
        assert_eq!(
            String::from_utf8(sent.body.clone()).unwrap(),
            case.expected_ting_body(),
            "{label}"
        );
    }
    for (label, gesture, case) in [
        (
            "show.dismissed: esc_triple",
            "esc_triple",
            show_dismissed(&ids, "esc_triple"),
        ),
        (
            "ask.dismissed: double_esc",
            "double_esc",
            ask_dismissed(&ids, "double_esc"),
        ),
        (
            "show.dismissed: EscDouble",
            "EscDouble",
            show_dismissed(&ids, "EscDouble"),
        ),
    ] {
        let message = refuse(&h, label, &case).await;
        assert!(
            message.contains(&format!("`{gesture}`")),
            "{label}: {message}"
        );
    }
    assert_eq!(
        Harness::requests(&h.ting, "/v1/tings").await.len(),
        accepted.len()
    );
}

#[tokio::test]
async fn the_allowlist_is_exactly_the_nine_peek_types() {
    assert_eq!(
        TingType::ALL.map(TingType::as_str),
        [
            "peek.ask.answered",
            "peek.ask.dismissed",
            "peek.ask.expired",
            "peek.message.received",
            "peek.speech.finished",
            "peek.show.dismissed",
            "peek.send.expired",
            "peek.schedule.due",
            "peek.send.shown",
        ],
        "registration order: the six 0.1.0 types, then the three 0.1.2 ones"
    );
    let h = Harness::start().await;
    ready(&h).await;
    mount_ting_send(&h.ting, 202, false).await;
    let ids = Ids::new();
    let [se, sd, ss] = new_types(&ids, "production");
    // (label, delivery, a fragment the refusal must name).
    let cases = [
        (
            "near miss: peek.send.expire",
            se.typed("peek.send.expire"),
            "`peek.send.expire`",
        ),
        (
            "near miss: peek.schedule.fired",
            sd.typed("peek.schedule.fired"),
            "`peek.schedule.fired`",
        ),
        (
            "near miss: peek.send.displayed",
            ss.typed("peek.send.displayed"),
            "`peek.send.displayed`",
        ),
        (
            "near miss: capitalised",
            ss.typed("Peek.Send.Shown"),
            "`Peek.Send.Shown`",
        ),
        (
            "not a peek type: peek.ask.replaced",
            ss.typed("peek.ask.replaced"),
            "`peek.ask.replaced`",
        ),
        // Another type's data never passes under a new name (strict schemas).
        (
            "send.shown data as peek.send.expired",
            ss.typed("peek.send.expired")
                .keyed(format!("{ACTOR}/{}/send_expired", ids.send)),
            "peek.send.expired data",
        ),
        (
            "send.expired data as peek.send.shown",
            se.typed("peek.send.shown")
                .keyed(format!("{ACTOR}/{}/shown", ids.send)),
            "peek.send.shown data",
        ),
        (
            "send.expired data as peek.speech.finished",
            se.typed("peek.speech.finished")
                .keyed(format!("{ACTOR}/{}/speech_finished", ids.send)),
            "peek.speech.finished data",
        ),
        (
            "schedule.due data as peek.ask.expired",
            sd.typed("peek.ask.expired")
                .keyed(format!("{ACTOR}/{}/expired", ids.ask)),
            "peek.ask.expired data",
        ),
    ];
    for (label, case, fragment) in &cases {
        let message = refuse(&h, label, case).await;
        assert!(message.contains(fragment), "{label}: {message}");
    }
    assert!(Harness::requests(&h.ting, "/v1/tings").await.is_empty());
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // one typed payload per 0.1.2 shape
async fn peekd_built_deliveries_are_accepted() {
    let h = Harness::start().await;
    ready(&h).await;
    mount_ting_send(&h.ting, 202, false).await;
    let actor = ActorId::parse(ACTOR).unwrap();
    let ts = |v: &str| Timestamp::parse(v).unwrap();
    let slot = SlotIndex::new(5).unwrap();
    let (send_id, ask_id, schedule_id) = (
        SendId::generate(),
        AskId::generate(),
        ScheduleId::generate(),
    );
    let queued_schedule_id = ScheduleId::generate();
    let deliveries = [
        (
            TingData::SendExpired(SendExpired {
                schema: SchemaV1,
                send_id: send_id.clone(),
                kind: "speak".into(),
                created_at: ts("2026-09-27T12:00:00Z"),
                expires_at: ts("2026-09-27T12:01:00Z"),
                expired_at: ts("2026-09-27T12:01:00.010Z"),
                shown: true,
                shown_at: Some(ts("2026-09-27T12:00:01Z")),
                scheduled: true,
                schedule_id: Some(schedule_id.clone()),
                slot,
                context: DataContext::Production,
            }),
            format!("{ACTOR}/{send_id}/send_expired"),
        ),
        (
            TingData::ScheduleDue(ScheduleDue {
                schema: SchemaV1,
                schedule_id: schedule_id.clone(),
                send_id: send_id.clone(),
                ask_id: None,
                kind: "speak".into(),
                due_at: ts("2026-09-27T12:00:00Z"),
                fired_at: ts("2026-09-27T12:00:00.004Z"),
                outcome: DueOutcome::Replaced,
                replaced_send_id: Some(SendId::generate()),
                waiting_reason: Some(WaitingReason::Paused),
                queue_position: None,
                expires_at: Some(ts("2026-09-27T12:01:00Z")),
                slot,
                context: DataContext::Production,
            }),
            format!("{ACTOR}/{schedule_id}/due"),
        ),
        (
            TingData::ScheduleDue(ScheduleDue {
                schema: SchemaV1,
                schedule_id: queued_schedule_id.clone(),
                send_id: send_id.clone(),
                ask_id: Some(ask_id.clone()),
                kind: "ask".into(),
                due_at: ts("2026-09-27T12:00:00Z"),
                fired_at: ts("2026-09-27T12:00:00Z"),
                outcome: DueOutcome::Queued,
                replaced_send_id: None,
                waiting_reason: Some(WaitingReason::QueueFull),
                queue_position: Some(7),
                expires_at: None,
                slot,
                context: DataContext::Production,
            }),
            format!("{ACTOR}/{queued_schedule_id}/due"),
        ),
        (
            TingData::SendShown(SendShown {
                schema: SchemaV1,
                send_id: send_id.clone(),
                ask_id: None,
                kind: "show".into(),
                created_at: ts("2026-09-27T12:00:00Z"),
                shown_at: ts("2026-09-27T12:00:00.300Z"),
                scheduled: false,
                schedule_id: None,
                slot,
                context: DataContext::Production,
            }),
            format!("{ACTOR}/{send_id}/shown"),
        ),
        (
            TingData::AskExpired(AskExpired {
                schema: SchemaV1,
                ask_id: ask_id.clone(),
                send_id: send_id.clone(),
                question: "Stand-up in 5?".into(),
                ask_type: AskType::Text,
                asked_at: ts("2026-09-27T12:00:00Z"),
                expired_at: ts("2026-09-27T12:10:00Z"),
                slot,
                context: DataContext::Production,
                shown: Some(false),
            }),
            format!("{ACTOR}/{ask_id}/expired"),
        ),
    ];
    for (i, (data, key)) in deliveries.iter().enumerate() {
        let d = DeliveryRequest::new(&actor, data, TingMetadata::new(Some("deliberate".into())))
            .unwrap();
        assert_eq!(&d.key, key, "{}", data.ting_type().as_str());
        let r = h.send(deliver_request(&d, &key_for(&d))).await;
        assert_eq!(
            r.status,
            200,
            "{}: {}",
            data.ting_type().as_str(),
            String::from_utf8_lossy(&r.body)
        );
        let sends = Harness::requests(&h.ting, "/v1/tings").await;
        let sent: Value = serde_json::from_slice(&sends[i].body).unwrap();
        assert_eq!(sent["type"], data.ting_type().as_str());
        assert_eq!(sent["key"], key.as_str());
        assert_eq!(sent["data"], d.data);
    }
    let sent: Value =
        serde_json::from_slice(&Harness::requests(&h.ting, "/v1/tings").await[4].body).unwrap();
    assert_eq!(
        sent["data"]["shown"], false,
        "peekd 0.1.2 always sends shown"
    );
}
