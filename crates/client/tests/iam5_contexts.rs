//! IAM5 context and explicit feature-permission recovery contracts.
#![cfg(feature = "runtime")]
#![allow(clippy::expect_used, clippy::unwrap_used)]
mod common;
use common::{fixture, session_body, write_slot};
use serde_json::{Value, json};
use silicon_peek_client::{
    ErrorCode, Secret,
    identity::Context,
    runtime::{
        RefreshPolicy, auth_block, authenticate_home,
        authorization::{Action, perform},
        fresh_session_with,
    },
    timestamp::unix_now,
};
use std::time::Duration;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

fn approval(complete: bool) -> Value {
    json!({"request_id":"4e8cc0d5-a4dd-477c-8e3a-5c2fa90ce2ec","completed":complete,"roots":[],
        "authorization":{"id":"68d52bca-bbca-4350-ac60-2d66229f31ef","app_id":"peek",
        "actor":{"type":"silicon","public_id":"si:cleanup"},"org_id":"tos","status":if complete {"exchanged"} else {"pending"},
        "version":1,"expires_at":"2099-01-01T00:00:00Z","authorization_url":"https://iam.example/consent","state":null,"endpoints":[]}})
}
#[tokio::test]
async fn approval_start_and_completion_recover_exact_requests_after_restart() {
    let server = MockServer::start().await;
    let f = fixture(&server.uri());
    write_slot(&f, "oat_live", "ort_live", unix_now() + 3600);
    let id = f
        .store
        .read_session()
        .unwrap()
        .slot(&f.key)
        .unwrap()
        .context_id()
        .unwrap()
        .to_owned();
    assert!(
        perform(&f.store, &f.client, &f.key, &id, Action::Status)
            .await
            .unwrap()
            .is_none()
    );
    Mock::given(method("POST"))
        .and(path("/api/v1/ting/authorization"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    assert!(
        perform(&f.store, &f.client, &f.key, &id, Action::Start)
            .await
            .is_err()
    );
    Mock::given(method("POST"))
        .and(path("/api/v1/ting/authorization"))
        .respond_with(ResponseTemplate::new(200).set_body_json(approval(false)))
        .mount(&server)
        .await;
    let reopened = silicon_peek_client::runtime::Store::open_existing(f.store.dir()).unwrap();
    perform(&reopened, &f.client, &f.key, &id, Action::Status)
        .await
        .unwrap();
    let completion = "/api/v1/ting/authorizations/4e8cc0d5-a4dd-477c-8e3a-5c2fa90ce2ec/complete";
    Mock::given(method("POST"))
        .and(path(completion))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    assert!(
        perform(
            &reopened,
            &f.client,
            &f.key,
            &id,
            Action::Complete(Some(Secret::new("approval-code")))
        )
        .await
        .is_err()
    );
    Mock::given(method("POST"))
        .and(path(completion))
        .respond_with(ResponseTemplate::new(200).set_body_json(approval(true)))
        .mount(&server)
        .await;
    assert!(
        perform(&reopened, &f.client, &f.key, &id, Action::Complete(None))
            .await
            .unwrap()
            .unwrap()
            .completed
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 4);
    for pair in requests.chunks(2) {
        assert_eq!(
            pair[0].headers["idempotency-key"],
            pair[1].headers["idempotency-key"]
        );
        assert_eq!(pair[0].body, pair[1].body);
    }
    assert!(
        !std::fs::read_to_string(reopened.path("feature-consent.json"))
            .unwrap()
            .contains("approval-code")
    );
}
#[tokio::test]
async fn changed_terms_require_new_review_and_stale_context_never_sends() {
    let server = MockServer::start().await;
    let f = fixture(&server.uri());
    write_slot(&f, "oat_live", "ort_live", unix_now() + 3600);
    let id = f
        .store
        .read_session()
        .unwrap()
        .slot(&f.key)
        .unwrap()
        .context_id()
        .unwrap()
        .to_owned();
    Mock::given(method("POST"))
        .and(path("/api/v1/ting/authorization"))
        .respond_with(ResponseTemplate::new(200).set_body_json(approval(false)))
        .mount(&server)
        .await;
    perform(&f.store, &f.client, &f.key, &id, Action::Start)
        .await
        .unwrap();
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/ting/authorizations/4e8cc0d5-a4dd-477c-8e3a-5c2fa90ce2ec/complete",
        ))
        .respond_with(ResponseTemplate::new(412))
        .mount(&server)
        .await;
    assert_eq!(
        perform(
            &f.store,
            &f.client,
            &f.key,
            &id,
            Action::Complete(Some(Secret::new("code")))
        )
        .await
        .unwrap_err()
        .status(),
        Some(412)
    );
    assert!(
        perform(&f.store, &f.client, &f.key, &id, Action::Complete(None))
            .await
            .is_err()
    );
    perform(&f.store, &f.client, &f.key, &id, Action::Start)
        .await
        .unwrap();
    let requests = server.received_requests().await.unwrap();
    assert_ne!(
        requests[0].headers["idempotency-key"],
        requests[2].headers["idempotency-key"]
    );
    let lock = f.store.lock().unwrap();
    let mut file = f.store.read_session().unwrap();
    file.slots
        .get_mut(&f.key.as_string())
        .unwrap()
        .extra
        .insert("context_id".into(), json!(uuid::Uuid::new_v4()));
    f.store.write_session(&lock, &file).unwrap();
    drop(lock);
    assert_eq!(
        *perform(&f.store, &f.client, &f.key, &id, Action::Start)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::SessionRejected
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}
#[tokio::test]
async fn refreshed_actor_org_and_world_must_stay_immutable() {
    for changed in ["actor", "org", "world"] {
        let server = MockServer::start().await;
        let f = fixture(&server.uri());
        write_slot(&f, "oat_old", "ort_old", 1);
        let mut value = session_body("oat_new", "ort_new", 3600);
        match changed {
            "actor" => {
                value["actor"]["public_id"] = json!("si:another");
                value["membership_id"] = json!("si:another[tos]");
            }
            "org" => {
                value["org_id"] = json!("other");
                value["org_ids"] = json!(["other"]);
                value["membership_id"] = json!("si:cleanup[other]");
            }
            _ => {
                value["testing_environment"] =
                    json!({"id":uuid::Uuid::new_v4(),"name":"other","generation":1});
            }
        }
        Mock::given(method("POST"))
            .and(path("/api/v1/auth/refresh"))
            .respond_with(ResponseTemplate::new(200).set_body_json(value))
            .expect(1)
            .mount(&server)
            .await;
        assert!(
            fresh_session_with(
                &f.store,
                &f.client,
                Context::Production,
                Duration::from_secs(60),
                &RefreshPolicy::single_attempt()
            )
            .await
            .is_err()
        );
        assert_eq!(
            f.store
                .read_session()
                .unwrap()
                .slot(&f.key)
                .unwrap()
                .refresh_token
                .expose(),
            "ort_old"
        );
    }
}
#[test]
fn old_ipc_requests_cannot_adopt_a_replacement_login() {
    let f = fixture("http://127.0.0.1:12345");
    write_slot(&f, "oat_old", "ort_old", unix_now() + 3600);
    let lock = f.store.lock().unwrap();
    f.store.ensure_daemon_token(&lock).unwrap();
    drop(lock);
    let auth = auth_block(&f.store, &f.key).unwrap();
    assert!(authenticate_home(&auth).is_ok());
    let lock = f.store.lock().unwrap();
    let mut file = f.store.read_session().unwrap();
    file.slots
        .get_mut(&f.key.as_string())
        .unwrap()
        .extra
        .insert("context_id".into(), json!(uuid::Uuid::new_v4()));
    f.store.write_session(&lock, &file).unwrap();
    drop(lock);
    assert_eq!(
        *authenticate_home(&auth).unwrap_err().code(),
        ErrorCode::SessionRejected
    );
}

#[test]
fn permission_responses_reject_wrong_identity_request_state_and_unsafe_links() {
    let valid: silicon_peek_client::authorization::TingAuthorization =
        serde_json::from_value(approval(false)).unwrap();
    for field in ["actor", "org", "request", "state", "url"] {
        let mut value = approval(false);
        match field {
            "actor" => value["authorization"]["actor"]["public_id"] = json!("si:other"),
            "org" => value["authorization"]["org_id"] = json!("other"),
            "request" => value["request_id"] = json!(uuid::Uuid::new_v4()),
            "state" => value["authorization"]["state"] = json!("unexpected"),
            _ => {
                value["authorization"]["authorization_url"] =
                    json!("https://user:secret@iam.example/consent");
            }
        }
        let changed: silicon_peek_client::authorization::TingAuthorization =
            serde_json::from_value(value).unwrap();
        assert!(
            changed
                .validate(
                    &valid.authorization.actor,
                    &valid.authorization.org_id,
                    Some(&valid)
                )
                .is_err(),
            "{field}"
        );
    }
}

#[tokio::test]
async fn decline_keeps_session_and_work_available_but_requires_new_review() {
    let server = MockServer::start().await;
    let f = fixture(&server.uri());
    write_slot(&f, "oat_live", "ort_live", unix_now() + 3600);
    let id = f
        .store
        .read_session()
        .unwrap()
        .slot(&f.key)
        .unwrap()
        .context_id()
        .unwrap()
        .to_owned();
    let mut declined = approval(false);
    declined["authorization"]["status"] = json!("declined");
    Mock::given(method("POST"))
        .and(path("/api/v1/ting/authorization"))
        .respond_with(ResponseTemplate::new(200).set_body_json(declined))
        .mount(&server)
        .await;
    perform(&f.store, &f.client, &f.key, &id, Action::Start)
        .await
        .unwrap();
    assert_eq!(
        *perform(
            &f.store,
            &f.client,
            &f.key,
            &id,
            Action::Complete(Some(Secret::new("code")))
        )
        .await
        .unwrap_err()
        .code(),
        ErrorCode::ReconsentRequired
    );
    assert!(
        perform(&f.store, &f.client, &f.key, &id, Action::Status)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        f.store
            .read_session()
            .unwrap()
            .usable_slot(&f.key, f.store.dir())
            .is_ok()
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}
