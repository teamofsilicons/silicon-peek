//! Login, recovery, logout, revocation and peekd home authentication against
//! a mock peek-server and temp homes (BLUEPRINT §1.6, §2.4, §2.6).

#![cfg(feature = "runtime")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use std::time::Duration;

use common::{error_body, fixture, secret, session_body, write_slot};
use silicon_peek_client::{
    ErrorCode, Secret,
    identity::Context,
    ids::IdempotencyKey,
    runtime::{
        auth_block, authenticate_home,
        login::{RemoteRevocation, login, logout, recover_login, revoke_pending},
        store::DAEMON_TOKEN_FILE,
    },
    timestamp::unix_now,
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_string, header, method, path},
};

const LOGIN: &str = "/api/v1/auth/login";
const LOGOUT: &str = "/api/v1/auth/logout";

fn fast() -> Vec<Duration> {
    vec![Duration::from_millis(10); 3]
}

#[tokio::test]
async fn login_commits_the_session_and_creates_the_daemon_token() {
    let server = MockServer::start().await;
    let key = IdempotencyKey::login("oac_slt");
    Mock::given(method("POST"))
        .and(path(LOGIN))
        .and(header("idempotency-key", key.as_str()))
        .and(body_string(r#"{"slt":"oac_slt"}"#))
        .respond_with(ResponseTemplate::new(503).set_body_json(error_body(
            "iam_unavailable",
            "down",
            true,
        )))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(LOGIN))
        .and(header("idempotency-key", key.as_str()))
        .and(body_string(r#"{"slt":"oac_slt"}"#))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(session_body("oat_1", "ort_1", 1800)),
        )
        .expect(1)
        .mount(&server)
        .await;
    let f = fixture(&server.uri());
    let before = unix_now();
    let out = login(
        &f.store,
        &f.client,
        Context::Production,
        &secret(" oac_slt\n"),
        None,
        &fast(),
    )
    .await
    .expect("login");
    assert_eq!(out.slot.actor.public_id.as_str(), "si:cleanup");
    assert!(
        out.slot.access_expires_at >= before + 1800
            && out.slot.access_expires_at <= unix_now() + 1800
    );
    assert!(out.replaced.is_none());
    assert_eq!(out.daemon_token.expose().len(), 64);
    assert_eq!(out.ting.as_ref().map(|t| t.subscribed), Some(true));
    let file = f.store.read_session().unwrap();
    assert!(file.pending_login.is_none());
    assert!(file.logged_out.is_none());
    assert_eq!(file.slot(&f.key).unwrap().refresh_token.expose(), "ort_1");
    assert_eq!(f.store.daemon_token().unwrap().unwrap(), out.daemon_token);
}

#[tokio::test]
async fn a_second_login_queues_the_old_family_for_revocation() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(LOGIN))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(session_body("oat_2", "ort_2", 1800)),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(LOGOUT))
        .and(header(
            "idempotency-key",
            IdempotencyKey::revoke("ort_1").as_str(),
        ))
        .and(body_string(r#"{"token":"ort_1"}"#))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let f = fixture(&server.uri());
    write_slot(&f, "oat_1", "ort_1", unix_now() + 1800);
    let out = login(
        &f.store,
        &f.client,
        Context::Production,
        &secret("oac_two"),
        None,
        &fast(),
    )
    .await
    .expect("login");
    assert_eq!(
        out.replaced.as_ref().map(|r| r.token.expose().to_owned()),
        Some("ort_1".into())
    );
    assert_eq!(f.store.read_session().unwrap().pending_revocations.len(), 1);
    let client = f.client.clone();
    let sweep = revoke_pending(&f.store, |_| Some(client.clone()))
        .await
        .unwrap();
    assert_eq!((sweep.confirmed, sweep.pending), (1, 0));
    assert!(
        f.store
            .read_session()
            .unwrap()
            .pending_revocations
            .is_empty()
    );
}

#[tokio::test]
async fn a_rejected_slt_clears_the_pending_login() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(LOGIN))
        .respond_with(ResponseTemplate::new(401).set_body_json(error_body(
            "slt_rejected",
            "expired or used",
            false,
        )))
        .expect(1)
        .mount(&server)
        .await;
    let f = fixture(&server.uri());
    let e = login(
        &f.store,
        &f.client,
        Context::Production,
        &secret("oac_bad"),
        None,
        &fast(),
    )
    .await
    .expect_err("rejected");
    assert_eq!(*e.code(), ErrorCode::SltRejected);
    assert_eq!(e.exit_code().code(), 3);
    assert!(f.store.read_session().unwrap().pending_login.is_none());
}

#[tokio::test]
async fn public_ids_are_refused_in_production_before_any_request() {
    let server = MockServer::start().await;
    let f = fixture(&server.uri());
    let e = login(
        &f.store,
        &f.client,
        Context::Production,
        &secret("si:tester"),
        None,
        &fast(),
    )
    .await
    .expect_err("public id");
    assert_eq!(*e.code(), ErrorCode::SltIsPublicId);
    assert_eq!(e.exit_code().code(), 2);
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn an_uncertain_login_can_be_recovered_with_the_same_key() {
    let server = MockServer::start().await;
    let key = IdempotencyKey::login("oac_slt");
    Mock::given(method("POST"))
        .and(path(LOGIN))
        .respond_with(ResponseTemplate::new(502))
        .up_to_n_times(4)
        .mount(&server)
        .await;
    let f = fixture(&server.uri());
    let e = login(
        &f.store,
        &f.client,
        Context::Production,
        &secret("oac_slt"),
        None,
        &fast(),
    )
    .await
    .expect_err("uncertain");
    assert!(e.retryable());
    assert!(e.hint().unwrap().contains("--recover"));
    let file = f.store.read_session().unwrap();
    let pending = file.pending_login.as_ref().expect("kept for --recover");
    assert_eq!(pending.key, key.as_str());
    assert_eq!(pending.slt.expose(), "oac_slt");

    Mock::given(method("POST"))
        .and(path(LOGIN))
        .and(header("idempotency-key", key.as_str()))
        .and(body_string(r#"{"slt":"oac_slt"}"#))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(session_body("oat_1", "ort_1", 1800)),
        )
        .expect(1)
        .mount(&server)
        .await;
    let out = recover_login(&f.store, &f.client, Context::Production, &fast())
        .await
        .expect("recovered");
    assert_eq!(out.slot.access_token.expose(), "oat_1");
    assert!(f.store.read_session().unwrap().pending_login.is_none());

    let e = recover_login(&f.store, &f.client, Context::Production, &fast())
        .await
        .expect_err("nothing pending");
    assert_eq!(*e.code(), ErrorCode::InvalidInput);
}

#[tokio::test]
async fn testing_public_id_logins_use_one_key_per_attempt() {
    // A testing environment's "SLT" is the Silicon's public ID, the same on
    // every login: a key derived from it alone makes IAM replay the first
    // (revoked) pair after a logout, then refuse the login for a day.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(LOGIN))
        .respond_with(ResponseTemplate::new(502))
        .up_to_n_times(4)
        .mount(&server)
        .await;
    let f = fixture(&server.uri());
    let ctx = Context::Testing(uuid::Uuid::now_v7());
    login(
        &f.store,
        &f.client,
        ctx,
        &secret("si:peek-tester"),
        None,
        &fast(),
    )
    .await
    .expect_err("uncertain");
    let first = f
        .store
        .read_session()
        .unwrap()
        .pending_login
        .expect("kept for --recover")
        .key;
    assert!(first.starts_with("peek-login-"));
    assert_ne!(first, IdempotencyKey::login("si:peek-tester").as_str());
    let keys = |requests: &[wiremock::Request]| -> Vec<String> {
        requests
            .iter()
            .map(|r| r.headers["idempotency-key"].to_str().unwrap().to_owned())
            .collect()
    };
    // In-process retries of one attempt keep its key.
    let sent = keys(&server.received_requests().await.unwrap());
    assert_eq!(sent.len(), 4);
    assert!(sent.iter().all(|k| *k == first), "{sent:?}");

    // --recover replays the same attempt with the same key.
    Mock::given(method("POST"))
        .and(path(LOGIN))
        .and(header("idempotency-key", first.as_str()))
        .and(body_string(r#"{"slt":"si:peek-tester"}"#))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(session_body("oat_1", "ort_1", 1800)),
        )
        .expect(1)
        .mount(&server)
        .await;
    recover_login(&f.store, &f.client, ctx, &fast())
        .await
        .expect("recovered");

    // A later login with the same public ID is a new attempt with a new key.
    server.reset().await;
    Mock::given(method("POST"))
        .and(path(LOGIN))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(session_body("oat_2", "ort_2", 1800)),
        )
        .mount(&server)
        .await;
    login(
        &f.store,
        &f.client,
        ctx,
        &secret("si:peek-tester"),
        None,
        &fast(),
    )
    .await
    .expect("second login");
    let second = keys(&server.received_requests().await.unwrap());
    assert_eq!(second.len(), 1);
    assert_ne!(second[0], first, "each login attempt has its own key");
    assert!(second[0].starts_with("peek-login-"));
}

#[tokio::test]
async fn recovery_after_the_replay_window_is_refused() {
    let server = MockServer::start().await;
    let f = fixture(&server.uri());
    f.store
        .update_session(|file| {
            file.pending_login = Some(silicon_peek_client::runtime::session::PendingLogin {
                key: IdempotencyKey::login("oac_old").as_str().to_owned(),
                slt: Secret::new("oac_old"),
                started_at: unix_now() - 601,
                slot: Some(f.key.as_string()),
                org_hint: None,
            });
            Ok(())
        })
        .unwrap();
    let e = recover_login(&f.store, &f.client, Context::Production, &fast())
        .await
        .expect_err("expired");
    assert_eq!(*e.code(), ErrorCode::LoginAttemptExpired);
    assert!(f.store.read_session().unwrap().pending_login.is_none());
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn logout_keeps_the_ting_grant_and_deletes_the_slot() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(LOGOUT))
        .and(body_string(r#"{"token":"ort_1"}"#))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let f = fixture(&server.uri());
    write_slot(&f, "oat_1", "ort_1", unix_now() + 1800);
    let out = logout(&f.store, &f.client, Context::Production, false)
        .await
        .expect("logout");
    assert_eq!(out.remote_revocation, RemoteRevocation::Confirmed);
    assert_eq!(out.actor.map(|a| a.to_string()), Some("si:cleanup".into()));
    let reqs = server.received_requests().await.unwrap();
    assert!(
        !reqs[0].headers.contains_key("authorization"),
        "without the bearer no backend revokes the shared Ting grant"
    );
    let file = f.store.read_session().unwrap();
    assert!(file.slot(&f.key).is_none());
    assert!(file.pending_revocations.is_empty());
    assert_eq!(
        file.logged_out.as_ref().map(|l| l.actor.as_str()),
        Some("si:cleanup")
    );
}

#[tokio::test]
async fn logout_with_revoke_ting_sends_the_bearer_and_the_flag() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(LOGOUT))
        .and(header("authorization", "Bearer oat_1"))
        .and(header("x-org-id", "tos"))
        .and(body_string(r#"{"token":"ort_1","revoke_ting":true}"#))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let f = fixture(&server.uri());
    write_slot(&f, "oat_1", "ort_1", unix_now() + 1800);
    let out = logout(&f.store, &f.client, Context::Production, true)
        .await
        .expect("logout");
    assert_eq!(out.remote_revocation, RemoteRevocation::Confirmed);
    assert!(f.store.read_session().unwrap().slot(&f.key).is_none());
}

#[tokio::test]
async fn an_unconfirmed_logout_still_logs_out_locally() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(LOGOUT))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let f = fixture(&server.uri());
    write_slot(&f, "oat_1", "ort_1", unix_now() + 1800);
    let out = logout(&f.store, &f.client, Context::Production, false)
        .await
        .expect("logout");
    assert_eq!(out.remote_revocation, RemoteRevocation::Pending);
    assert!(out.error.is_some());
    let file = f.store.read_session().unwrap();
    assert!(file.slot(&f.key).is_none(), "local logout always completes");
    assert_eq!(file.pending_revocations.len(), 1);
    assert_eq!(file.pending_revocations[0].token.expose(), "ort_1");
    // Logging out again is a no-op.
    let again = logout(&f.store, &f.client, Context::Production, false)
        .await
        .expect("again");
    assert!(again.actor.is_none());
}

#[tokio::test]
async fn peekd_authenticates_homes_by_token_and_slot() {
    let server = MockServer::start().await;
    let f = fixture(&server.uri());
    write_slot(&f, "oat_1", "ort_1", unix_now() + 1800);
    let lock = f.store.lock().unwrap();
    f.store.ensure_daemon_token(&lock).unwrap();
    drop(lock);

    let auth = auth_block(&f.store, &f.key).unwrap();
    let verified = authenticate_home(&auth).expect("verified");
    assert_eq!(verified.actor_id.as_str(), "si:cleanup");
    assert_eq!(verified.org_id.as_str(), "tos");

    let mut wrong = auth.clone();
    wrong.home_token = Secret::new("f".repeat(64));
    let e = authenticate_home(&wrong).expect_err("token");
    assert_eq!(*e.code(), ErrorCode::HomeTokenMismatch);

    let mut other_ctx = auth.clone();
    other_ctx.context = Context::Testing(uuid::Uuid::now_v7());
    assert_eq!(
        *authenticate_home(&other_ctx).expect_err("slot").code(),
        ErrorCode::NotLoggedIn
    );

    let mut elsewhere = auth.clone();
    elsewhere.home = f.dir.path().join("missing").to_string_lossy().into_owned();
    assert_eq!(
        *authenticate_home(&elsewhere).expect_err("home").code(),
        ErrorCode::InvalidSiliconHome
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(f.store.dir(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            *authenticate_home(&auth).expect_err("mode").code(),
            ErrorCode::InvalidSiliconHome
        );
        std::fs::set_permissions(f.store.dir(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    f.store
        .update_session(|file| {
            file.slots.values_mut().next().unwrap().rejected =
                Some(silicon_peek_client::runtime::session::Rejection {
                    code: "session_rejected".into(),
                    at: unix_now(),
                    request_id: None,
                });
            Ok(())
        })
        .unwrap();
    assert_eq!(
        *authenticate_home(&auth).expect_err("rejected").code(),
        ErrorCode::SessionRejected
    );

    std::fs::remove_file(f.store.path(DAEMON_TOKEN_FILE)).unwrap();
    assert_eq!(
        *authenticate_home(&auth).expect_err("no token").code(),
        ErrorCode::NotLoggedIn
    );
}
