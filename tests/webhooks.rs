//! `POST /webhooks/iam`: signature verification over the raw body, dedupe,
//! removal cleanup and test-envelope routing.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use axum::{body::Body, http::Request};
use common::*;
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const SCRIPT: &str = "export default function draw() {}";

fn removal_event(event_id: Uuid, actor: &str, org: &str) -> Value {
    json!({
        "spec_version": "1.0", "event_id": event_id, "event_type": "organization.membership.removed.v1",
        "occurred_at": "2026-09-26T10:00:00Z", "organization_id": Uuid::now_v7(),
        "aggregate": {"type": "organization_membership", "id": Uuid::now_v7(), "version": 2},
        "data": {"changed_fields": ["membership.status"], "current": {"members": [{
            "resource": {"type": "organization_membership", "id": Uuid::now_v7(), "principal_id": actor,
                         "principal_type": "silicon", "membership_id": format!("{actor}[{org}]"), "version": 2, "status": "removed"},
            "authorization": "removed"
        }]}}
    })
}

fn test_envelope(event: &Value, key: &str) -> Value {
    let mut metadata = event.clone();
    let data = metadata.as_object_mut().unwrap().remove("data").unwrap();
    json!({"test": {"testing_key": key, "metadata": metadata, "data": data}})
}

fn signed(body: &[u8], event_id: Uuid, timestamp: i64, secret: &str) -> Request<Body> {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    let signature = hex::encode(mac.finalize().into_bytes());
    Request::post("/webhooks/iam")
        .header("content-type", "application/json")
        .header("x-silicon-iam-event-id", event_id.to_string())
        .header("x-silicon-iam-timestamp", timestamp.to_string())
        .header("x-silicon-iam-key-version", "1")
        .header("x-silicon-iam-signature", format!("v1={signature}"))
        .body(Body::from(body.to_vec()))
        .unwrap()
}

fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

async fn put_drawing(h: &Harness, testing_generation: Option<u64>) -> u16 {
    let mut b = authed("PUT", "/api/v1/drawings/current", ACCESS)
        .header("content-type", "application/javascript")
        .header(
            "x-peek-drawing-sha256",
            hex::encode(Sha256::digest(SCRIPT.as_bytes())),
        );
    if testing_generation.is_some() {
        b = testing(b, testing_generation);
    }
    h.send(b.body(Body::from(SCRIPT)).unwrap())
        .await
        .status
        .as_u16()
}

async fn get_drawing(h: &Harness, testing_plane: bool) -> u16 {
    let mut b = authed("GET", "/api/v1/drawings/current", ACCESS);
    if testing_plane {
        b = testing(b, None);
    }
    h.send(b.body(Body::empty()).unwrap()).await.status.as_u16()
}

#[tokio::test]
async fn a_removal_deletes_the_actors_data_once() {
    let h = Harness::start().await;
    mount_silicon(&h.iam, ACCESS).await;
    assert_eq!(put_drawing(&h, None).await, 200);
    let id = Uuid::now_v7();
    let body = serde_json::to_vec(&removal_event(id, ACTOR, ORG)).unwrap();
    let r = h.send(signed(&body, id, now(), WEBHOOK_SECRET)).await;
    assert_eq!(r.status, 204, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(
        get_drawing(&h, false).await,
        404,
        "the drawing went with the membership"
    );

    // A redelivery is acknowledged but not applied again.
    assert_eq!(put_drawing(&h, None).await, 200);
    let r = h.send(signed(&body, id, now(), WEBHOOK_SECRET)).await;
    assert_eq!(r.status, 204);
    assert_eq!(get_drawing(&h, false).await, 200, "duplicates are ignored");
    let received: Vec<_> = h
        .events()
        .into_iter()
        .filter(|e| e["event"] == "webhook.received")
        .collect();
    assert_eq!(received[0]["context"]["status"], "applied");
    assert_eq!(received[1]["context"]["status"], "duplicate");
}

#[tokio::test]
async fn forged_or_stale_deliveries_are_refused() {
    let h = Harness::start().await;
    mount_silicon(&h.iam, ACCESS).await;
    assert_eq!(put_drawing(&h, None).await, 200);
    let id = Uuid::now_v7();
    let body = serde_json::to_vec(&removal_event(id, ACTOR, ORG)).unwrap();

    let r = h
        .send(signed(
            &body,
            id,
            now(),
            "whs_wrongSecret_0123456789abcdef0123456789",
        ))
        .await;
    assert_eq!(r.status, 401);
    assert_eq!(r.code(), "unauthenticated");

    let r = h
        .send(signed(&body, id, now() - 3600, WEBHOOK_SECRET))
        .await;
    assert_eq!(r.status, 401, "outside the 5-minute tolerance");

    let mut tampered = body.clone();
    let last = tampered.len() - 2;
    tampered[last] = b' ';
    let mut req = signed(&body, id, now(), WEBHOOK_SECRET);
    *req.body_mut() = Body::from(tampered);
    let r = h.send(req).await;
    assert_eq!(r.status, 401, "the signature covers the exact bytes");

    let r = h
        .send(signed(&body, Uuid::now_v7(), now(), WEBHOOK_SECRET))
        .await;
    assert_eq!(
        r.status, 400,
        "the header event ID must match the signed body"
    );

    assert_eq!(get_drawing(&h, false).await, 200, "nothing was applied");
}

#[tokio::test]
async fn other_events_are_acknowledged() {
    let h = Harness::start().await;
    let id = Uuid::now_v7();
    let mut event = removal_event(id, ACTOR, ORG);
    event["event_type"] = json!("organization.membership.updated.v1");
    let body = serde_json::to_vec(&event).unwrap();
    let r = h.send(signed(&body, id, now(), WEBHOOK_SECRET)).await;
    assert_eq!(r.status, 204);
}

#[tokio::test]
async fn test_envelopes_only_touch_their_environment() {
    let h = Harness::start().await;
    mount_silicon(&h.iam, ACCESS).await;
    mount_testing_contexts(&h.iam).await;
    mount_introspect_testing(
        &h.iam,
        ACCESS,
        introspection(ACTOR, ORG, &FULL_SCOPES, Some("admin"), Some(env_id())),
    )
    .await;
    h.prepare_environment().await;
    assert_eq!(put_drawing(&h, None).await, 200);
    assert_eq!(put_drawing(&h, Some(1)).await, 200);

    // An envelope for an environment peek does not know: acknowledged, ignored.
    let id = Uuid::now_v7();
    let unknown = serde_json::to_vec(&test_envelope(
        &removal_event(id, ACTOR, ORG),
        "Unknown0123456789Unknown01234567",
    ))
    .unwrap();
    let r = h.send(signed(&unknown, id, now(), WEBHOOK_SECRET)).await;
    assert_eq!(r.status, 204);
    assert_eq!(get_drawing(&h, true).await, 200);

    let id = Uuid::now_v7();
    let body =
        serde_json::to_vec(&test_envelope(&removal_event(id, ACTOR, ORG), ROOT_KEY)).unwrap();
    let r = h.send(signed(&body, id, now(), WEBHOOK_SECRET)).await;
    assert_eq!(r.status, 204, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(get_drawing(&h, true).await, 404, "the testing copy is gone");
    assert_eq!(get_drawing(&h, false).await, 200, "production is untouched");
}
