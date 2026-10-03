//! IAM 5 feature consent: real HTTP/SQLite with isolated IAM/Ting stand-ins.
#![allow(clippy::unwrap_used, clippy::expect_used)]
mod common;
use axum::{body::Body, http::Request};
use common::*;
use serde_json::{Value, json};
use silicon_peek_client::ids::{EventId, SendId};
use tower::ServiceExt;
use uuid::Uuid;
use wiremock::{
    Mock, ResponseTemplate,
    matchers::{body_string_contains, method, path},
};

fn permission(path: &str, body: &Value, key: &str) -> Request<Body> {
    json_body(
        authed("POST", path, ACCESS).header("idempotency-key", key),
        body,
    )
}
fn delivery() -> Value {
    let send = SendId::generate();
    json!({"event_id":EventId::generate(),"type":"peek.show.dismissed","key":format!("{ACTOR}/{send}/show_dismissed"),"data":{"schema":1,"send_id":send,"gesture":"esc_double","visible_ms":900,"dismissed_at":"2026-09-27T12:00:01Z","slot":3,"context":"production"},"metadata":{"peek_version":"0.1.6"}})
}
async fn send(h: &Harness, body: &Value, key: &str) -> Response {
    h.send(permission("/api/v1/deliveries", body, key)).await
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // one complete consent and secret-storage lifecycle
async fn permission_is_manual_bound_to_the_original_context_and_encrypted() {
    let h = Harness::start().await;
    mount_silicon(&h.iam, ACCESS).await;
    let denied = send(&h, &delivery(), "delivery-needs-permission").await;
    assert_eq!(
        denied.status,
        403,
        "{}",
        String::from_utf8_lossy(&denied.body)
    );
    assert_eq!(denied.json()["error"]["details"]["feature"], "ting");
    assert!(Harness::requests(&h.ting, "/v1/tings").await.is_empty());
    mount_exchange(&h.iam, None).await;
    let begin = h
        .send(permission(
            "/api/v1/ting/authorization",
            &json!({}),
            "same-start-permission-key",
        ))
        .await;
    assert_eq!(
        begin.status,
        200,
        "{}",
        String::from_utf8_lossy(&begin.body)
    );
    let id = begin.json()["request_id"].as_str().unwrap().to_owned();
    let again = h
        .send(permission(
            "/api/v1/ting/authorization",
            &json!({}),
            "same-start-permission-key",
        ))
        .await;
    assert_eq!(again.json()["request_id"], id);
    assert_eq!(
        Harness::requests(&h.iam, "/api/v1/obo-access/authorizations")
            .await
            .len(),
        1
    );
    mount_introspect(
        &h.iam,
        "oat_otherAccount",
        introspection("c:other", ORG, &FULL_SCOPES, None, None),
    )
    .await;
    let other = h
        .send(
            authed(
                "GET",
                &format!("/api/v1/ting/authorizations/{id}"),
                "oat_otherAccount",
            )
            .body(Body::empty())
            .unwrap(),
        )
        .await;
    assert_eq!(
        other.status, 404,
        "requests are not enumerable by another account"
    );
    let complete_path = format!("/api/v1/ting/authorizations/{id}/complete");
    let done = h
        .send(permission(
            &complete_path,
            &json!({"code":"manual-approval-code"}),
            "complete-permission-key",
        ))
        .await;
    assert_eq!(done.status, 200, "{}", String::from_utf8_lossy(&done.body));
    assert_eq!(done.json()["completed"], true);
    assert_eq!(done.json()["roots"].as_array().unwrap().len(), 3);
    let replay = h
        .send(permission(
            &complete_path,
            &json!({"code":"manual-approval-code"}),
            "another-complete-retry-key",
        ))
        .await;
    assert_eq!(replay.status, 200);
    assert_eq!(
        Harness::requests(&h.iam, "/api/v1/obo-access/tokens")
            .await
            .len(),
        1
    );
    let changed = h
        .send(permission(
            &complete_path,
            &json!({"code":"another-code"}),
            "complete-another-code-key",
        ))
        .await;
    assert_eq!(changed.status, 409);
    for name in ["peek.sqlite", "peek.sqlite-wal"] {
        let bytes = std::fs::read(h.dir.path().join(name)).unwrap_or_default();
        let text = String::from_utf8_lossy(&bytes);
        for secret in [
            ACCESS,
            "manual-approval-code",
            "oba_fixture_",
            "obr_fixture_",
        ] {
            assert!(!text.contains(secret), "plaintext authority in {name}");
        }
    }
    mount_ting_send(&h.ting, 202, false).await;
    let body = delivery();
    assert_eq!(
        send(&h, &body, "permission-delivery-stable-key")
            .await
            .status,
        200
    );
    let next = delivery();
    assert_eq!(
        send(&h, &next, "permission-another-delivery-key")
            .await
            .status,
        200
    );
    let sends = Harness::requests(&h.ting, "/v1/tings").await;
    assert_eq!(
        sends[0].headers["authorization"], sends[1].headers["authorization"],
        "reusable root, not per-call proof"
    );
    assert!(
        Harness::requests(&h.iam, "/api/v1/obo-access/exchanges")
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn decline_does_not_issue_tokens_or_invalidate_ordinary_login() {
    let h = Harness::start().await;
    mount_silicon(&h.iam, ACCESS).await;
    mount_exchange(&h.iam, None).await;
    let start = h
        .send(permission(
            "/api/v1/ting/authorization",
            &json!({}),
            "decline-request-key",
        ))
        .await;
    let auth = Uuid::parse_str(start.json()["authorization"]["id"].as_str().unwrap()).unwrap();
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/obo-access/authorizations/{auth}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(consent(auth, "declined")))
        .with_priority(1)
        .mount(&h.iam)
        .await;
    let id = start.json()["request_id"].as_str().unwrap().to_owned();
    let done = h
        .send(permission(
            &format!("/api/v1/ting/authorizations/{id}/complete"),
            &json!({"code":"declined-code"}),
            "declined-complete-key",
        ))
        .await;
    assert_eq!(done.status, 403);
    assert!(
        Harness::requests(&h.iam, "/api/v1/obo-access/tokens")
            .await
            .is_empty()
    );
    let me = h
        .send(
            authed("GET", "/api/v1/auth/me", ACCESS)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(me.status, 200);
    assert_eq!(me.json()["reconsent_required"], false);
}

#[tokio::test]
async fn lost_code_response_recovers_with_the_same_request_even_after_iam_exchanged_it() {
    let h = Harness::start().await;
    mount_silicon(&h.iam, ACCESS).await;
    mount_exchange(&h.iam, None).await;
    let start = h
        .send(permission(
            "/api/v1/ting/authorization",
            &json!({}),
            "lost-code-start-key",
        ))
        .await;
    let auth = Uuid::parse_str(start.json()["authorization"]["id"].as_str().unwrap()).unwrap();
    let id = start.json()["request_id"].as_str().unwrap().to_owned();
    let target = format!("/api/v1/ting/authorizations/{id}/complete");
    Mock::given(method("POST"))
        .and(path("/api/v1/obo-access/tokens"))
        .respond_with(iam_error(503, "uncertain"))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&h.iam)
        .await;
    assert_eq!(
        h.send(permission(
            &target,
            &json!({"code":"recover-code"}),
            "lost-code-complete-key"
        ))
        .await
        .status,
        503
    );
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/obo-access/authorizations/{auth}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(consent(auth, "exchanged")))
        .with_priority(1)
        .mount(&h.iam)
        .await;
    let done = h
        .send(permission(
            &target,
            &json!({"code":"recover-code"}),
            "retry-lost-complete-key",
        ))
        .await;
    assert_eq!(done.status, 200, "{}", String::from_utf8_lossy(&done.body));
    let calls = Harness::requests(&h.iam, "/api/v1/obo-access/tokens").await;
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].body, calls[1].body);
    assert_eq!(
        calls[0].headers["idempotency-key"],
        calls[1].headers["idempotency-key"]
    );
}

#[tokio::test]
async fn pending_delivery_cannot_move_when_provider_consent_selects_another_account() {
    let h = Harness::start().await;
    mount_silicon(&h.iam, ACCESS).await;
    mount_exchange(&h.iam, None).await;
    assert_eq!(h.approve_ting(ACCESS, false).await.status, 200);
    Mock::given(method("POST"))
        .and(path("/v1/tings"))
        .respond_with(ting_error(503, "unavailable"))
        .up_to_n_times(1)
        .mount(&h.ting)
        .await;
    let body = delivery();
    assert_eq!(send(&h, &body, "original-delivery-key").await.status, 503);
    Mock::given(method("POST")).and(path("/api/v1/obo-access/tokens")).and(body_string_contains("authorization_code")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"items":(["subscriptions.register","subscriptions.revoke","tings.send"].map(|endpoint|{let mut pair=root_pair(endpoint,None);pair["org_id"]=json!("other");pair["actor"]=json!({"type":"carbon","public_id":"c:receiver"});pair}))}))).with_priority(1).mount(&h.iam).await;
    assert_eq!(h.approve_ting(ACCESS, false).await.status, 200);
    let stale = send(&h, &body, "original-delivery-key").await;
    assert_eq!(
        stale.status,
        409,
        "{}",
        String::from_utf8_lossy(&stale.body)
    );
    assert_eq!(
        Harness::requests(&h.ting, "/v1/tings").await.len(),
        1,
        "no retargeted request"
    );
    mount_ting_send(&h.ting, 202, false).await;
    let fresh = send(&h, &delivery(), "new-provider-delivery-key").await;
    assert_eq!(
        fresh.status,
        200,
        "{}",
        String::from_utf8_lossy(&fresh.body)
    );
    let sends = Harness::requests(&h.ting, "/v1/tings").await;
    let selected: Value = serde_json::from_slice(&sends[1].body).unwrap();
    assert_eq!(selected["org_id"], "other");
    assert_eq!(selected["for"], "c:receiver");
}

#[tokio::test]
async fn uncertain_root_rotation_survives_restart_and_preserves_bytes_and_retry_key() {
    let mut h = Harness::start().await;
    mount_silicon(&h.iam, ACCESS).await;
    mount_exchange(&h.iam, None).await;
    assert_eq!(h.approve_ting(ACCESS, false).await.status, 200);
    Mock::given(method("POST"))
        .and(path("/v1/tings"))
        .respond_with(ting_error(401, "invalid_obo_token"))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&h.ting)
        .await;
    mount_ting_send(&h.ting, 202, false).await;
    Mock::given(method("POST"))
        .and(path("/api/v1/obo-access/tokens"))
        .and(body_string_contains("refresh_token"))
        .respond_with(iam_error(503, "uncertain_refresh"))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&h.iam)
        .await;
    let body = delivery();
    let first = send(&h, &body, "restart-provider-delivery-key").await;
    assert_eq!(first.status, 503);
    h.state = silicon_peek::state::AppState::new(h.state.config().clone(), None).unwrap();
    h.app = silicon_peek::app::router(h.state.clone());
    let second = send(&h, &body, "restart-provider-delivery-key").await;
    assert_eq!(
        second.status,
        200,
        "{}",
        String::from_utf8_lossy(&second.body)
    );
    let calls = Harness::requests(&h.iam, "/api/v1/obo-access/tokens").await;
    let refresh: Vec<_> = calls
        .iter()
        .filter(|c| String::from_utf8_lossy(&c.body).contains("refresh_token"))
        .collect();
    assert_eq!(refresh.len(), 2);
    assert_eq!(refresh[0].body, refresh[1].body);
    assert_eq!(
        refresh[0].headers["idempotency-key"],
        refresh[1].headers["idempotency-key"]
    );
    let sends = Harness::requests(&h.ting, "/v1/tings").await;
    assert_eq!(sends.len(), 2);
    assert_eq!(sends[0].body, sends[1].body);
    assert_eq!(
        sends[1].headers["authorization"],
        "Bearer oba_rotated_tings.send"
    );
}

#[tokio::test]
async fn changed_graph_keeps_ordinary_login_and_does_not_install_partial_authority() {
    let h = Harness::start().await;
    mount_silicon(&h.iam, ACCESS).await;
    mount_exchange(&h.iam, None).await;
    Mock::given(method("POST"))
        .and(path("/api/v1/obo-access/tokens"))
        .respond_with(iam_error(412, "obo_graph_changed"))
        .with_priority(1)
        .mount(&h.iam)
        .await;
    let complete = h.approve_ting(ACCESS, false).await;
    assert_eq!(complete.status, 412);
    assert_eq!(complete.json()["error"]["details"]["graph_changed"], true);
    assert_eq!(
        send(&h, &delivery(), "changed-graph-delivery-key")
            .await
            .status,
        403
    );
    let me = h
        .send(
            authed("GET", "/api/v1/auth/me", ACCESS)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(me.status, 200);
    assert!(Harness::requests(&h.ting, "/v1/tings").await.is_empty());
}

#[tokio::test]
async fn mismatched_rotated_family_never_reaches_the_provider() {
    let h = Harness::start().await;
    mount_silicon(&h.iam, ACCESS).await;
    mount_exchange(&h.iam, None).await;
    assert_eq!(h.approve_ting(ACCESS, false).await.status, 200);
    Mock::given(method("POST"))
        .and(path("/v1/tings"))
        .respond_with(ting_error(401, "invalid_obo_token"))
        .mount(&h.ting)
        .await;
    let mut changed = root_pair("tings.send", None);
    changed["actor"]["public_id"] = json!("si:another");
    Mock::given(method("POST"))
        .and(path("/api/v1/obo-access/tokens"))
        .and(body_string_contains("refresh_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"items":[changed]})))
        .with_priority(1)
        .mount(&h.iam)
        .await;
    let response = send(&h, &delivery(), "changed-family-delivery-key").await;
    assert_eq!(response.status, 502);
    assert_eq!(response.code(), "iam_unavailable");
    assert_eq!(Harness::requests(&h.ting, "/v1/tings").await.len(), 1);
}

#[tokio::test]
async fn concurrent_authorization_starts_serialize_across_server_instances() {
    let h = Harness::start().await;
    mount_silicon(&h.iam, ACCESS).await;
    mount_exchange(&h.iam, None).await;
    let auth = Uuid::new_v4();
    Mock::given(method("POST"))
        .and(path("/api/v1/obo-access/authorizations"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(consent(auth, "pending"))
                .set_delay(std::time::Duration::from_millis(100)),
        )
        .with_priority(1)
        .mount(&h.iam)
        .await;
    // A second application process opens the same SQLite state and sees the
    // durable lease, not just an in-process mutex.
    let state = silicon_peek::state::AppState::new(h.state.config().clone(), None).unwrap();
    let router = silicon_peek::app::router(state);
    let first = h.send(permission(
        "/api/v1/ting/authorization",
        &json!({}),
        "concurrent-start-one-key",
    ));
    let second = async {
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        router
            .oneshot(permission(
                "/api/v1/ting/authorization",
                &json!({}),
                "concurrent-start-two-key",
            ))
            .await
            .unwrap()
            .status()
    };
    let (first, second) = tokio::join!(first, second);
    assert_eq!(first.status, 200);
    assert_eq!(second, 409);
    assert_eq!(
        Harness::requests(&h.iam, "/api/v1/obo-access/authorizations")
            .await
            .len(),
        1
    );
}

#[tokio::test]
async fn testing_clean_erases_feature_authority_without_touching_production() {
    let h = Harness::start().await;
    mount_silicon(&h.iam, ACCESS).await;
    mount_exchange(&h.iam, None).await;
    let production = h.approve_ting(ACCESS, false).await;
    assert_eq!(production.status, 200);
    mount_testing_contexts(&h.iam).await;
    h.prepare_environment().await;
    let token = "oat_testFeatureAccess01";
    mount_introspect_testing(
        &h.iam,
        token,
        introspection(ACTOR, ORG, &FULL_SCOPES, Some("admin"), Some(env_id())),
    )
    .await;
    mount_exchange_for(
        &h.iam,
        TEST_SECRET,
        Some(json!({"app_id":"ting","app_secret":TING_TEST_SECRET,"iam_test_key":ROOT_KEY})),
    )
    .await;
    let approved = h.approve_ting(token, true).await;
    assert_eq!(approved.status, 200);
    let id = approved.json()["request_id"].as_str().unwrap().to_owned();
    let clean = h
        .send(participant_request(&operation("clean", 2, 2, 1, ROOT_KEY)))
        .await;
    assert_eq!(clean.status, 200);
    let stale = h
        .send(
            testing(
                authed("GET", &format!("/api/v1/ting/authorizations/{id}"), token),
                None,
            )
            .body(Body::empty())
            .unwrap(),
        )
        .await;
    assert_eq!(stale.status, 404, "the old permission request was erased");
    let enroll = h
        .send(json_body(
            testing(
                authed("POST", "/api/v1/ting/recipient", token)
                    .header("idempotency-key", "new-generation-enroll-key"),
                Some(2),
            ),
            &json!({}),
        ))
        .await;
    assert_eq!(enroll.status, 403, "clean removed the approved roots");
    mount_ting_send(&h.ting, 202, false).await;
    assert_eq!(
        send(&h, &delivery(), "production-survives-clean-key")
            .await
            .status,
        200
    );
}
