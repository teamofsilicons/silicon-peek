//! `fresh_session` against a mock peek-server (BLUEPRINT §2.5).

#![cfg(feature = "runtime")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use std::time::Duration;

use common::{error_body, fixture, secret, session_body, write_slot};
use silicon_peek_client::{
    ErrorCode,
    identity::Context,
    ids::IdempotencyKey,
    runtime::{RefreshPolicy, force_refresh, fresh_session, fresh_session_with},
    timestamp::unix_now,
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_string, header, method, path},
};

const REFRESH: &str = "/api/v1/auth/refresh";

fn fast() -> RefreshPolicy {
    RefreshPolicy::with_delays(vec![Duration::from_millis(20); 3])
}

#[tokio::test]
async fn a_fresh_session_is_returned_without_network() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(REFRESH))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let f = fixture(&server.uri());
    write_slot(&f, "oat_live", "ort_1", unix_now() + 1800);
    let slot = fresh_session(
        &f.store,
        &f.client,
        Context::Production,
        Duration::from_secs(60),
    )
    .await
    .expect("fresh");
    assert_eq!(slot.access_token.expose(), "oat_live");
}

#[tokio::test]
async fn an_expiring_session_rotates_with_the_deterministic_key_and_exact_body() {
    let server = MockServer::start().await;
    let key = IdempotencyKey::refresh("ort_1");
    Mock::given(method("POST"))
        .and(path(REFRESH))
        .and(header("idempotency-key", key.as_str()))
        .and(body_string(r#"{"refresh_token":"ort_1"}"#))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(session_body("oat_2", "ort_2", 1800)),
        )
        .expect(1)
        .mount(&server)
        .await;
    let f = fixture(&server.uri());
    write_slot(&f, "oat_old", "ort_1", unix_now() + 30);
    let before = unix_now();
    let slot = fresh_session(
        &f.store,
        &f.client,
        Context::Production,
        Duration::from_secs(60),
    )
    .await
    .expect("refreshed");
    assert_eq!(slot.access_token.expose(), "oat_2");
    assert_eq!(slot.refresh_token.expose(), "ort_2");
    assert!(slot.access_expires_at >= before + 1800 && slot.access_expires_at <= unix_now() + 1800);
    assert!(slot.pending_refresh_key.is_none() && slot.refresh_started_at.is_none());
    let stored = f.store.read_session().unwrap();
    let stored = stored.slot(&f.key).unwrap();
    assert_eq!(stored.refresh_token.expose(), "ort_2");
    assert!(stored.pending_refresh_key.is_none());
}

#[tokio::test]
async fn server_errors_are_retried_with_the_same_key_and_body() {
    let server = MockServer::start().await;
    let key = IdempotencyKey::refresh("ort_1");
    Mock::given(method("POST"))
        .and(path(REFRESH))
        .and(header("idempotency-key", key.as_str()))
        .and(body_string(r#"{"refresh_token":"ort_1"}"#))
        .respond_with(ResponseTemplate::new(503).set_body_json(error_body(
            "iam_unavailable",
            "IAM is down",
            true,
        )))
        .up_to_n_times(2)
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(REFRESH))
        .and(header("idempotency-key", key.as_str()))
        .and(body_string(r#"{"refresh_token":"ort_1"}"#))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(session_body("oat_2", "ort_2", 1800)),
        )
        .expect(1)
        .mount(&server)
        .await;
    let f = fixture(&server.uri());
    write_slot(&f, "oat_old", "ort_1", unix_now() - 5);
    let slot = fresh_session_with(
        &f.store,
        &f.client,
        Context::Production,
        Duration::from_secs(60),
        &fast(),
    )
    .await
    .expect("recovered");
    assert_eq!(slot.access_token.expose(), "oat_2");
}

#[tokio::test]
async fn the_pending_key_is_persisted_before_io_and_survives_exhausted_retries() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(REFRESH))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "3"))
        .expect(1)
        .mount(&server)
        .await;
    let f = fixture(&server.uri());
    write_slot(&f, "oat_old", "ort_1", unix_now() - 5);
    let e = fresh_session_with(
        &f.store,
        &f.client,
        Context::Production,
        Duration::from_secs(60),
        &RefreshPolicy::single_attempt(),
    )
    .await
    .expect_err("rate limited");
    assert!(e.retryable());
    assert_eq!(e.exit_code().code(), 5);
    assert_eq!(e.retry_after(), Some(Duration::from_secs(3)));
    let file = f.store.read_session().unwrap();
    let slot = file.slot(&f.key).unwrap();
    assert_eq!(
        slot.pending_refresh_key.as_deref(),
        Some(IdempotencyKey::refresh("ort_1").as_str())
    );
    assert!(slot.refresh_started_at.is_some());
    assert!(slot.rejected.is_none(), "an outage is never a rejection");
}

#[tokio::test]
async fn a_resumed_refresh_reuses_the_stored_key_and_start_time() {
    let server = MockServer::start().await;
    let key = IdempotencyKey::refresh("ort_1");
    Mock::given(method("POST"))
        .and(path(REFRESH))
        .and(header("idempotency-key", key.as_str()))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(session_body("oat_2", "ort_2", 1800)),
        )
        .expect(1)
        .mount(&server)
        .await;
    let f = fixture(&server.uri());
    write_slot(&f, "oat_old", "ort_1", unix_now() - 5);
    let started = unix_now() - 120;
    f.store
        .update_session(|file| {
            let s = file.slots.values_mut().next().unwrap();
            s.pending_refresh_key = Some(key.as_str().to_owned());
            s.refresh_started_at = Some(started);
            Ok(())
        })
        .unwrap();
    let slot = fresh_session(
        &f.store,
        &f.client,
        Context::Production,
        Duration::from_secs(60),
    )
    .await
    .expect("resumed");
    assert_eq!(
        slot.access_expires_at,
        started + 1800,
        "expiry counts from the original start"
    );
}

#[tokio::test]
async fn a_401_is_terminal_and_marks_the_slot() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(REFRESH))
        .respond_with(ResponseTemplate::new(401).set_body_json(error_body(
            "session_rejected",
            "refresh token reused",
            false,
        )))
        .expect(1)
        .mount(&server)
        .await;
    let f = fixture(&server.uri());
    write_slot(&f, "oat_old", "ort_1", unix_now() - 5);
    let e = fresh_session_with(
        &f.store,
        &f.client,
        Context::Production,
        Duration::from_secs(60),
        &fast(),
    )
    .await
    .expect_err("rejected");
    assert_eq!(*e.code(), ErrorCode::SessionRejected);
    assert_eq!(e.exit_code().code(), 3);
    assert_eq!(e.request_id(), Some("req_1"));
    let file = f.store.read_session().unwrap();
    let slot = file.slot(&f.key).unwrap();
    let r = slot.rejected.as_ref().expect("rejection recorded");
    assert_eq!(r.code, "session_rejected");
    assert_eq!(r.request_id.as_deref(), Some("req_1"));
    assert!(slot.pending_refresh_key.is_none());
    // Later calls fail fast, without the network (the mock expects one call).
    let e = fresh_session(
        &f.store,
        &f.client,
        Context::Production,
        Duration::from_secs(60),
    )
    .await
    .expect_err("still rejected");
    assert_eq!(*e.code(), ErrorCode::SessionRejected);
}

#[tokio::test]
async fn an_expired_replay_window_is_terminal() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(REFRESH))
        .respond_with(ResponseTemplate::new(409).set_body_json(error_body(
            "idempotency_response_expired",
            "too late",
            false,
        )))
        .expect(1)
        .mount(&server)
        .await;
    let f = fixture(&server.uri());
    write_slot(&f, "oat_old", "ort_1", unix_now() - 5);
    let e = fresh_session_with(
        &f.store,
        &f.client,
        Context::Production,
        Duration::from_secs(60),
        &fast(),
    )
    .await
    .expect_err("terminal");
    assert_eq!(*e.code(), ErrorCode::SessionRejected);
    let file = f.store.read_session().unwrap();
    assert_eq!(
        file.slot(&f.key)
            .unwrap()
            .rejected
            .as_ref()
            .map(|r| r.code.as_str()),
        Some("idempotency_response_expired")
    );
}

#[tokio::test]
async fn iam_misconfigured_is_an_outage_not_a_rejection() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(REFRESH))
        .respond_with(ResponseTemplate::new(503).set_body_json(error_body(
            "iam_misconfigured",
            "invalid_client",
            true,
        )))
        .mount(&server)
        .await;
    let f = fixture(&server.uri());
    write_slot(&f, "oat_old", "ort_1", unix_now() - 5);
    let e = fresh_session_with(
        &f.store,
        &f.client,
        Context::Production,
        Duration::from_secs(60),
        &fast(),
    )
    .await
    .expect_err("outage");
    assert_eq!(*e.code(), ErrorCode::IamMisconfigured);
    assert_eq!(e.exit_code().code(), 5);
    let file = f.store.read_session().unwrap();
    let slot = file.slot(&f.key).unwrap();
    assert!(slot.rejected.is_none());
    assert!(slot.pending_refresh_key.is_some());
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        4,
        "1 attempt + 3 retries"
    );
}

#[tokio::test]
async fn transport_failures_keep_the_refresh_pending() {
    // Nothing listens on this port.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let f = fixture(&format!("http://127.0.0.1:{port}"));
    write_slot(&f, "oat_old", "ort_1", unix_now() - 5);
    let e = fresh_session_with(
        &f.store,
        &f.client,
        Context::Production,
        Duration::from_secs(60),
        &RefreshPolicy::single_attempt(),
    )
    .await
    .expect_err("unreachable");
    assert_eq!(*e.code(), ErrorCode::BackendUnavailable);
    assert!(e.is_transport());
    let file = f.store.read_session().unwrap();
    assert!(file.slot(&f.key).unwrap().pending_refresh_key.is_some());
}

#[tokio::test]
async fn concurrent_refreshes_of_one_family_rotate_once() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(REFRESH))
        .and(header(
            "idempotency-key",
            IdempotencyKey::refresh("ort_1").as_str(),
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(session_body("oat_2", "ort_2", 1800))
                .set_delay(Duration::from_millis(300)),
        )
        .expect(1)
        .mount(&server)
        .await;
    let f = fixture(&server.uri());
    write_slot(&f, "oat_old", "ort_1", unix_now() - 5);
    let (s1, c1) = (f.store.clone(), f.client.clone());
    let (s2, c2) = (f.store.clone(), f.client.clone());
    let a = tokio::spawn(async move {
        fresh_session(&s1, &c1, Context::Production, Duration::from_secs(60)).await
    });
    let b = tokio::spawn(async move {
        fresh_session(&s2, &c2, Context::Production, Duration::from_secs(60)).await
    });
    let (a, b) = (a.await.unwrap().expect("a"), b.await.unwrap().expect("b"));
    assert_eq!(a.access_token.expose(), "oat_2");
    assert_eq!(b.access_token.expose(), "oat_2");
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn force_refresh_rotates_only_the_rejected_token() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(REFRESH))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(session_body("oat_2", "ort_2", 1800)),
        )
        .expect(1)
        .mount(&server)
        .await;
    let f = fixture(&server.uri());
    write_slot(&f, "oat_1", "ort_1", unix_now() + 1800);
    let policy = RefreshPolicy::single_attempt();
    let slot = force_refresh(
        &f.store,
        &f.client,
        Context::Production,
        &secret("oat_1"),
        &policy,
    )
    .await
    .expect("rotated");
    assert_eq!(slot.access_token.expose(), "oat_2");
    // A second caller that saw the old token gets the new one without I/O.
    let slot = force_refresh(
        &f.store,
        &f.client,
        Context::Production,
        &secret("oat_1"),
        &policy,
    )
    .await
    .expect("already rotated");
    assert_eq!(slot.access_token.expose(), "oat_2");
}

#[tokio::test]
async fn missing_sessions_are_not_logged_in() {
    let server = MockServer::start().await;
    let f = fixture(&server.uri());
    let e = fresh_session(
        &f.store,
        &f.client,
        Context::Production,
        Duration::from_secs(60),
    )
    .await
    .expect_err("no session");
    assert_eq!(*e.code(), ErrorCode::NotLoggedIn);
    assert_eq!(e.exit_code().code(), 3);
    assert!(e.hint().is_some_and(|h| h.contains("peek login")));
}
