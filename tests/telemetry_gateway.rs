//! `POST /api/web/telemetry` (BLUEPRINT §6.3) against a fake Space Station.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use axum::http::Request;
use common::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use silicon_peek::state::Limits;
use uuid::Uuid;
use wiremock::{
    Mock, ResponseTemplate,
    matchers::{method, path},
};

fn batch(table: &str, n: usize) -> Value {
    let events: Vec<Value> = (0..n)
        .map(|i| {
            json!({"id": format!("evt-{i}"), "type": "command.finished",
                   "data": {"source": "cli", "context": {"command": "send", "speak_text": "secret words"}},
                   "metadata": {"occurred_at": "2026-09-26T10:00:00Z"}})
        })
        .collect();
    json!({"table": table, "events": events})
}

fn relay() -> axum::http::request::Builder {
    Request::post("/api/web/telemetry")
        .header("content-type", "application/json")
        .header("x-peek-source", "cli")
        .header("user-agent", "peek-cli/0.1.0")
}

async fn mount_ingest(h: &Harness) {
    Mock::given(method("POST"))
        .and(path("/api/ingest"))
        .respond_with(|req: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
            ResponseTemplate::new(200)
                .set_body_json(json!({"batch_id": body["batch_id"], "status": "ok"}))
        })
        .mount(&h.spacestation)
        .await;
}

#[tokio::test]
async fn relays_allowlisted_tables_with_stable_record_ids() {
    let h = Harness::start().await;
    mount_ingest(&h).await;
    let body = batch("peekclidaemon", 2);
    let r = h.send(json_body(relay(), &body)).await;
    assert_eq!(r.status, 204, "{}", String::from_utf8_lossy(&r.body));
    let ingests = Harness::requests(&h.spacestation, "/api/ingest").await;
    assert_eq!(ingests.len(), 1);
    let sent: Value = serde_json::from_slice(&ingests[0].body).unwrap();
    assert!(Uuid::parse_str(sent["batch_id"].as_str().unwrap()).is_ok());
    let records = sent["records"].as_array().unwrap();
    assert_eq!(records.len(), 2);
    let key = format!("table-peekclidaemon-{}", "c".repeat(32));
    let r0 = &records[0];
    assert_eq!(r0["key"], key);
    assert_eq!(r0["metadata"]["table_id"], "peekclidaemon");
    assert_eq!(r0["metadata"]["event_ts_ms"], 1_790_416_800_000_i64);
    let digest = Sha256::digest(format!("{key}:evt-0").as_bytes());
    let expected = Uuid::from_bytes(digest[..16].try_into().unwrap());
    assert_eq!(r0["metadata"]["record_id"], expected.to_string());
    assert_eq!(r0["record"]["type"], "command.finished");
    assert_eq!(
        r0["record"]["data"]["context"],
        json!({"command": "send"}),
        "context is allowlisted"
    );
    assert_eq!(r0["record"]["metadata"]["server"]["client_reported"], true);
    assert_eq!(r0["record"]["metadata"]["server"]["source"], "cli");
    assert_eq!(
        r0["record"]["metadata"]["server"]["user_agent"],
        "peek-cli/0.1.0"
    );
    assert!(!String::from_utf8_lossy(&ingests[0].body).contains("secret words"));
    assert!(
        h.events().is_empty(),
        "the gateway records no http.completed about itself"
    );
}

#[tokio::test]
async fn the_0_1_2_queue_context_keys_survive_the_scrub() {
    let h = Harness::start().await;
    mount_ingest(&h).await;
    let body = json!({"table": "peekclidaemon", "events": [
        {"id": "evt-queued", "type": "send.queued",
         "data": {"source": "peekd", "context": {"slot": 3, "queue_waiting": 2, "scheduled": false, "speak_text": "secret words"}},
         "metadata": {"occurred_at": "2026-09-27T12:00:00Z"}},
        {"id": "evt-fired", "type": "schedule.fired",
         "data": {"source": "peekd", "context": {"slot": 3, "status": "queued", "scheduled": true, "summary": "Stand-up in 5?"}},
         "metadata": {"occurred_at": "2026-09-27T12:30:00Z"}}
    ]});
    let r = h.send(json_body(relay(), &body)).await;
    assert_eq!(r.status, 204, "{}", String::from_utf8_lossy(&r.body));
    let ingests = Harness::requests(&h.spacestation, "/api/ingest").await;
    assert_eq!(ingests.len(), 1);
    let sent: Value = serde_json::from_slice(&ingests[0].body).unwrap();
    let records = sent["records"].as_array().unwrap();
    assert_eq!(
        records[0]["record"]["data"]["context"],
        json!({"slot": 3, "queue_waiting": 2, "scheduled": false}),
        "queue_waiting and scheduled are allowlisted (contract §3.11)"
    );
    assert_eq!(
        records[1]["record"]["data"]["context"],
        json!({"slot": 3, "status": "queued", "scheduled": true})
    );
    let raw = String::from_utf8_lossy(&ingests[0].body);
    assert!(!raw.contains("secret words") && !raw.contains("Stand-up"));
    assert!(
        silicon_peek_client::telemetry::CONTEXT_KEYS.contains(&"queue_waiting")
            && silicon_peek_client::telemetry::CONTEXT_KEYS.contains(&"scheduled")
    );
}

#[tokio::test]
async fn refuses_other_tables_bad_batches_and_foreign_origins() {
    let h = Harness::start().await;
    mount_ingest(&h).await;
    let body = batch("peekbackend", 1);
    let r = h.send(json_body(relay(), &body)).await;
    assert_eq!(r.status, 403);
    assert_eq!(r.code(), "telemetry_table_unavailable");

    let body = batch("peekclidaemon", 41);
    let r = h.send(json_body(relay(), &body)).await;
    assert_eq!(r.status, 400, "at most 40 events");
    let body = batch("peekclidaemon", 0);
    let r = h.send(json_body(relay(), &body)).await;
    assert_eq!(r.status, 400, "at least one event");

    let mut body = batch("peekclidaemon", 1);
    body["events"][0]["data"]["blob"] = json!("x".repeat(64 * 1024));
    let r = h.send(json_body(relay(), &body)).await;
    assert_eq!(r.status, 413, "at most 64 KiB");

    let body = batch("peekfrontendanalytics", 1);
    let r = h
        .send(json_body(
            relay().header("origin", "https://evil.example"),
            &body,
        ))
        .await;
    assert_eq!(r.status, 403);
    assert_eq!(r.code(), "origin_not_allowed");
    let r = h
        .send(json_body(
            relay().header("origin", "https://peek.teamofsilicons.com"),
            &body,
        ))
        .await;
    assert_eq!(r.status, 204);
    assert_eq!(
        Harness::requests(&h.spacestation, "/api/ingest")
            .await
            .len(),
        1
    );
}

#[tokio::test]
async fn opt_outs_and_missing_keys_drop_silently() {
    let h = Harness::start().await;
    mount_ingest(&h).await;
    let body = batch("peekclidaemon", 1);
    let r = h
        .send(json_body(relay().header("x-peek-telemetry", "off"), &body))
        .await;
    assert_eq!(r.status, 204);
    let r = h
        .send(json_body(
            relay().header("x-testing-environment-key", TEST_SECRET),
            &body,
        ))
        .await;
    assert_eq!(
        r.status, 204,
        "test telemetry never reaches the production tables"
    );
    let events = batch("peekfrontendevents", 1);
    let r = h.send(json_body(relay(), &events)).await;
    assert_eq!(r.status, 204, "no key for peekfrontendevents: dropped");
    let mut tagged = batch("peekclidaemon", 1);
    tagged["events"][0]["data"]["environment"] = json!("testing");
    let r = h.send(json_body(relay(), &tagged)).await;
    assert_eq!(r.status, 204);
    assert!(
        Harness::requests(&h.spacestation, "/api/ingest")
            .await
            .is_empty()
    );

    let off = Harness::with(
        |env| {
            env.insert("PEEK_TELEMETRY", "off".to_owned());
        },
        Limits::default(),
    )
    .await;
    mount_ingest(&off).await;
    let r = off.send(json_body(relay(), &body)).await;
    assert_eq!(r.status, 204);
    assert!(
        Harness::requests(&off.spacestation, "/api/ingest")
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn the_gateway_is_rate_limited_and_reports_rejections() {
    let h = Harness::with(
        |_| {},
        Limits {
            telemetry_events_per_minute: 3,
            ..Limits::default()
        },
    )
    .await;
    Mock::given(method("POST"))
        .and(path("/api/ingest"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "batch_id": "b", "status": "rejected",
            "rejected": [{"record_id": Uuid::nil(), "code": "duplicate", "reason": "seen"}]
        })))
        .mount(&h.spacestation)
        .await;
    let body = batch("peekclidaemon", 2);
    assert_eq!(
        h.send(json_body(relay(), &body)).await.status,
        204,
        "duplicates count as accepted"
    );
    let r = h.send(json_body(relay(), &body)).await;
    assert_eq!(r.status, 429);
    assert_eq!(r.code(), "rate_limited");
}
