//! The Stemcell contract against a wiremock peek-server: `peek login`
//! (idempotency key, exact body, retries, store, recovery), every
//! `peek login status` case of BLUEPRINT §2.6, `peek logout`, testing
//! environments and `peek ting enroll`.

mod common;

use common::{DEAD_API, Env, error_body, me_body, session_body};
use serde_json::json;
use silicon_peek_client::timestamp::unix_now;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_string, header, header_exists, method, path},
};

fn login_key(slt: &str) -> String {
    format!("peek-login-{}", blake3::hash(slt.as_bytes()).to_hex())
}

fn refresh_key(rt: &str) -> String {
    format!("peek-refresh-{}", blake3::hash(rt.as_bytes()).to_hex())
}

#[tokio::test]
async fn login_exchanges_the_slt_and_writes_the_store() {
    let server = MockServer::start().await;
    let token = "oac_test_short_lived_token";
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login"))
        .and(header("idempotency-key", login_key(token).as_str()))
        .and(header("content-type", "application/json"))
        .and(header("peek-client-version", env!("CARGO_PKG_VERSION")))
        .and(header("x-org-id", "tos"))
        .and(body_string(format!(r#"{{"slt":"{token}"}}"#)))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(session_body("oat_one", "ort_one", 1800)),
        )
        .expect(1)
        .mount(&server)
        .await;
    let mut env = Env::new(&server.uri());
    env.var("SILICON_ORG", "tos");
    let before = unix_now();
    let run = env.run(&["login", token, "--json"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    let v = run.json();
    assert_eq!(v["authenticated"], true);
    assert_eq!(v["id"], "si:cleanup");
    assert_eq!(
        v["actor"],
        json!({"type": "silicon", "public_id": "si:cleanup"})
    );
    assert_eq!(v["org_id"], "tos");
    assert_eq!(v["membership_id"], "si:cleanup[tos]");
    assert_eq!(v["authority"], "silicon");
    assert_eq!(v["custody"], "client");
    assert_eq!(v["scopes"].as_array().map(Vec::len), Some(6));
    assert_eq!(v["ting"]["subscribed"], true);
    assert_eq!(v["daemon"]["attached"], false);
    assert_eq!(v["validated"], true);
    assert_eq!(v["api_url"], server.uri());
    assert!(v["testing_environment_id"].is_null());
    assert!(
        !run.stdout.contains("oat_one") && !run.stdout.contains("ort_one"),
        "no tokens printed"
    );
    let file = env.session();
    let slot = file
        .slot(&env.slot_key())
        .cloned()
        .unwrap_or_else(|| panic!("slot written"));
    assert_eq!(slot.access_token.expose(), "oat_one");
    assert_eq!(slot.refresh_token.expose(), "ort_one");
    assert!(slot.access_expires_at >= before + 1800 && slot.access_expires_at <= unix_now() + 1800);
    assert!(file.pending_login.is_none());
    let token = std::fs::read_to_string(env.store_dir().join("daemon-token")).unwrap_or_default();
    assert_eq!(token.trim().len(), 64);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = |p: &std::path::Path| {
            std::fs::metadata(p).map_or(0, |m| m.permissions().mode() & 0o777)
        };
        assert_eq!(mode(&env.store_dir()), 0o700);
        assert_eq!(mode(&env.store_dir().join("session.json")), 0o600);
        assert_eq!(mode(&env.store_dir().join("daemon-token")), 0o600);
    }
}

#[tokio::test]
async fn login_retries_a_5xx_with_the_same_key_and_body() {
    let server = MockServer::start().await;
    let token = "oac_retry_me";
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login"))
        .and(header("idempotency-key", login_key(token).as_str()))
        .respond_with(ResponseTemplate::new(503).set_body_json(error_body(
            "backend_unavailable",
            "down",
            true,
        )))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login"))
        .and(header("idempotency-key", login_key(token).as_str()))
        .and(body_string(format!(r#"{{"slt":"{token}"}}"#)))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(session_body("oat_two", "ort_two", 1800)),
        )
        .expect(1)
        .mount(&server)
        .await;
    let env = Env::new(&server.uri());
    let run = env.run(&["login", "--json", token]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.json()["authenticated"], true);
}

#[tokio::test]
async fn login_reads_the_slt_from_stdin() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login"))
        .and(body_string(r#"{"slt":"oac_from_stdin"}"#))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(session_body("oat_s", "ort_s", 1800)),
        )
        .expect(1)
        .mount(&server)
        .await;
    let env = Env::new(&server.uri());
    let run = env
        .run_stdin(
            &["login", "--token-file", "-", "--json"],
            Some(b"oac_from_stdin\n"),
        )
        .await;
    assert_eq!(run.code, 0, "{}", run.stderr);
}

#[tokio::test]
async fn a_rejected_slt_is_exit_3_and_clears_the_attempt() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login"))
        .respond_with(ResponseTemplate::new(401).set_body_json(error_body(
            "slt_rejected",
            "the SLT was already used",
            false,
        )))
        .expect(1)
        .mount(&server)
        .await;
    let env = Env::new(&server.uri());
    let run = env.run(&["login", "oac_used", "--json"]).await;
    assert_eq!(run.code, 3);
    assert!(run.stdout.is_empty());
    let e = run.error();
    assert_eq!(e["code"], "slt_rejected");
    assert_eq!(e["request_id"], "req_1");
    assert!(env.session().pending_login.is_none());
}

#[tokio::test]
async fn a_public_id_is_not_an_slt_in_production() {
    let server = MockServer::start().await;
    let env = Env::new(&server.uri());
    let run = env.run(&["login", "si:cleanup", "--json"]).await;
    assert_eq!(run.code, 2);
    assert_eq!(run.error()["code"], "slt_is_public_id");
    assert!(
        server
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty()
    );
}

#[tokio::test]
async fn recover_replays_the_pending_login() {
    let server = MockServer::start().await;
    let token = "oac_interrupted";
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login"))
        .and(header("idempotency-key", login_key(token).as_str()))
        .and(body_string(format!(r#"{{"slt":"{token}"}}"#)))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(session_body("oat_r", "ort_r", 1800)),
        )
        .expect(1)
        .mount(&server)
        .await;
    let env = Env::new(&server.uri());
    let store = env.store();
    let started = unix_now() - 30;
    let key = env.slot_key().as_string();
    store
        .update_session(|f| {
            f.pending_login = serde_json::from_value(json!({
                "key": login_key(token), "slt": token, "started_at": started, "slot": key, "org_hint": null
            }))
            .ok();
            Ok(())
        })
        .unwrap_or_else(|e| panic!("{e}"));
    let run = env.run(&["login", "--recover", "--json"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    let slot = env
        .session()
        .slot(&env.slot_key())
        .cloned()
        .unwrap_or_else(|| panic!("slot"));
    assert_eq!(
        slot.access_expires_at,
        started + 1800,
        "expiry counts from the first attempt"
    );
}

#[tokio::test]
async fn recover_after_ten_minutes_is_login_attempt_expired() {
    let server = MockServer::start().await;
    let env = Env::new(&server.uri());
    let key = env.slot_key().as_string();
    env.store()
        .update_session(|f| {
            f.pending_login = serde_json::from_value(json!({
                "key": login_key("oac_old"), "slt": "oac_old", "started_at": unix_now() - 700, "slot": key
            }))
            .ok();
            Ok(())
        })
        .unwrap_or_else(|e| panic!("{e}"));
    let run = env.run(&["login", "--recover", "--json"]).await;
    assert_eq!(run.code, 3);
    assert_eq!(run.error()["code"], "login_attempt_expired");
    assert!(
        server
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty()
    );
}

#[tokio::test]
async fn status_without_a_store_is_no_session() {
    let env = Env::new(DEAD_API);
    let run = env.run(&["login", "status", "--json"]).await;
    assert_eq!(run.code, 0);
    assert_eq!(
        run.json(),
        json!({"authenticated": false, "id": null, "reason": "no_session",
               "api_url": DEAD_API, "testing_environment_id": null})
    );
}

#[tokio::test]
async fn status_after_logout_is_logged_out() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/logout"))
        .and(header(
            "idempotency-key",
            format!("peek-revoke-{}", blake3::hash(b"ort_x").to_hex()).as_str(),
        ))
        .and(body_string(r#"{"token":"ort_x"}"#))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let env = Env::new(&server.uri());
    env.login_as("oat_x", "ort_x", unix_now() + 1800);
    let out = env.run(&["logout", "--json"]).await;
    assert_eq!(out.code, 0, "{}", out.stderr);
    let reqs = server.received_requests().await.unwrap_or_default();
    assert!(
        !reqs[0].headers.contains_key("authorization"),
        "a plain logout keeps the Ting grant other homes of the Silicon use"
    );
    assert_eq!(
        out.json(),
        json!({"authenticated": false, "remote_revocation": "confirmed"})
    );
    let file = env.session();
    assert!(file.slot(&env.slot_key()).is_none());
    assert!(file.logged_out.is_some());
    assert!(file.pending_revocations.is_empty());
    let status = env.run(&["login", "status", "--json"]).await;
    assert_eq!(status.code, 0);
    assert_eq!(status.json()["reason"], "logged_out");
    assert_eq!(status.json()["authenticated"], false);
}

#[tokio::test]
async fn logout_revoke_ting_sends_the_flag_and_the_bearer() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/logout"))
        .and(header("authorization", "Bearer oat_x"))
        .and(header("x-org-id", "tos"))
        .and(body_string(r#"{"token":"ort_x","revoke_ting":true}"#))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let env = Env::new(&server.uri());
    env.login_as("oat_x", "ort_x", unix_now() + 1800);
    let out = env.run(&["logout", "--revoke-ting", "--json"]).await;
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(
        out.json(),
        json!({"authenticated": false, "remote_revocation": "confirmed"})
    );
}

#[tokio::test]
async fn logout_with_the_backend_down_is_pending_but_local() {
    let env = Env::new(DEAD_API);
    env.login_as("oat_y", "ort_y", unix_now() + 1800);
    let run = env.run(&["logout", "--json"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.json()["remote_revocation"], "pending");
    let file = env.session();
    assert!(file.slot(&env.slot_key()).is_none());
    assert_eq!(file.pending_revocations.len(), 1);
    let human = env.run(&["logout"]).await;
    assert_eq!(human.code, 0);
}

#[tokio::test]
async fn status_of_a_rejected_session_is_exit_0_with_next() {
    let env = Env::new(DEAD_API);
    let store = env.login_as("oat_z", "ort_z", unix_now() + 1800);
    let key = env.slot_key().as_string();
    store
        .update_session(|f| {
            if let Some(s) = f.slots.get_mut(&key) {
                s.rejected = serde_json::from_value(
                    json!({"code":"session_rejected","at":unix_now(),"request_id":"req_9"}),
                )
                .ok();
            }
            Ok(())
        })
        .unwrap_or_else(|e| panic!("{e}"));
    let run = env.run(&["login", "status", "--json"]).await;
    assert_eq!(run.code, 0);
    let v = run.json();
    assert_eq!(v["authenticated"], false);
    assert!(v["id"].is_null());
    assert_eq!(v["reason"], "rejected");
    assert_eq!(v["rejection"]["code"], "session_rejected");
    assert_eq!(v["rejection"]["request_id"], "req_9");
    assert_eq!(
        v["next"],
        "si auth setup peek   (or: iam silicon-login … --app-id peek …; peek login '<SLT>')"
    );
}

#[tokio::test]
async fn status_verifies_live_and_reports_the_blueprint_shape() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/auth/me"))
        .and(header("authorization", "Bearer oat_live"))
        .and(header("x-org-id", "tos"))
        .respond_with(ResponseTemplate::new(200).set_body_json(me_body()))
        .expect(1)
        .mount(&server)
        .await;
    let env = Env::new(&server.uri());
    env.login_as("oat_live", "ort_live", unix_now() + 1800);
    let run = env.run(&["login", "status", "--json"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    let v = run.json();
    for k in [
        "authenticated",
        "id",
        "actor",
        "org_id",
        "org_ids",
        "membership_id",
        "authority",
        "custody",
        "scopes",
        "reconsent_required",
        "access_expires_at",
        "logged_in_at",
        "family_expires_at_estimate",
        "refresh_pending",
        "ting",
        "daemon",
        "testing_environment_id",
        "api_url",
        "store",
        "validated",
    ] {
        assert!(v.get(k).is_some(), "missing {k}");
    }
    assert_eq!(v["authenticated"], true);
    assert_eq!(v["validated"], true);
    assert_eq!(v["refresh_pending"], false);
    assert_eq!(
        v["daemon"],
        json!({"attached": false, "queued_answers": 0, "authority_required": 0})
    );
    assert!(
        v["access_expires_at"]
            .as_str()
            .is_some_and(|t| t.ends_with('Z'))
    );
    assert_eq!(v["store"], env.store_dir().display().to_string());
    assert!(
        env.session()
            .slot(&env.slot_key())
            .is_some_and(|s| s.verified_at.is_some())
    );
}

#[tokio::test]
async fn status_answers_the_blueprint_shape_when_silicon_org_is_not_the_sessions() {
    // Stemcell sets SILICON_ORG on every app call and reads `login status
    // --json` before it re-grants or removes peek after an org switch: status
    // must still answer (exit 0, a boolean `authenticated`), verified against
    // the session's own org, and say which requested org it does not cover.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/auth/me"))
        .and(header("authorization", "Bearer oat_live"))
        .and(header("x-org-id", "tos"))
        .respond_with(ResponseTemplate::new(200).set_body_json(me_body()))
        .expect(2)
        .mount(&server)
        .await;
    let mut env = Env::new(&server.uri());
    env.login_as("oat_live", "ort_live", unix_now() + 1800);
    env.var("SILICON_ORG", "acme");
    let run = env.run(&["login", "status", "--json"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    let v = run.json();
    assert_eq!(v["authenticated"], true);
    assert_eq!(v["org_id"], "tos");
    assert_eq!(v["org_ids"], json!(["tos"]));
    assert_eq!(v["requested_org"], "acme");
    assert_eq!(v["requested_org_authorized"], false);
    // --org behaves the same, in human mode too (with a hint on stderr).
    let run = env.run(&["--org", "acme", "login", "status"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(run.stderr.contains("not `acme`"), "{}", run.stderr);
    // Bearer commands still refuse an org the session does not cover.
    let other = env.run(&["org", "byo", "deepgram", "show", "--json"]).await;
    assert_eq!(other.code, 2, "{}", other.stderr);
}

#[tokio::test]
async fn status_refreshes_an_expiring_token_with_the_derived_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/refresh"))
        .and(header("idempotency-key", refresh_key("ort_old").as_str()))
        .and(body_string(r#"{"refresh_token":"ort_old"}"#))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(session_body("oat_new", "ort_new", 1800)),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/auth/me"))
        .and(header("authorization", "Bearer oat_new"))
        .respond_with(ResponseTemplate::new(200).set_body_json(me_body()))
        .expect(1)
        .mount(&server)
        .await;
    let env = Env::new(&server.uri());
    env.login_as("oat_old", "ort_old", unix_now() + 10);
    let run = env.run(&["login", "status", "--json"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.json()["authenticated"], true);
    let slot = env
        .session()
        .slot(&env.slot_key())
        .cloned()
        .unwrap_or_else(|| panic!("slot"));
    assert_eq!(slot.refresh_token.expose(), "ort_new");
    assert!(slot.pending_refresh_key.is_none());
}

#[tokio::test]
async fn a_401_from_me_forces_one_refresh_and_retries() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/auth/me"))
        .and(header("authorization", "Bearer oat_revoked"))
        .respond_with(ResponseTemplate::new(401).set_body_json(error_body(
            "unauthenticated",
            "inactive token",
            false,
        )))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/refresh"))
        .and(header("idempotency-key", refresh_key("ort_r1").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(session_body(
            "oat_fresh",
            "ort_r2",
            1800,
        )))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/auth/me"))
        .and(header("authorization", "Bearer oat_fresh"))
        .respond_with(ResponseTemplate::new(200).set_body_json(me_body()))
        .expect(1)
        .mount(&server)
        .await;
    let env = Env::new(&server.uri());
    env.login_as("oat_revoked", "ort_r1", unix_now() + 1800);
    let run = env.run(&["login", "status", "--json"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.json()["authenticated"], true);
}

#[tokio::test]
async fn a_second_401_is_rejected_with_exit_0() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/auth/me"))
        .respond_with(ResponseTemplate::new(401).set_body_json(error_body(
            "unauthenticated",
            "inactive token",
            false,
        )))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/refresh"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(session_body("oat_b", "ort_b2", 1800)),
        )
        .expect(1)
        .mount(&server)
        .await;
    let env = Env::new(&server.uri());
    env.login_as("oat_a", "ort_b1", unix_now() + 1800);
    let run = env.run(&["login", "status", "--json"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    let v = run.json();
    assert_eq!(v["authenticated"], false);
    assert_eq!(v["reason"], "rejected");
    assert!(
        env.session()
            .slot(&env.slot_key())
            .is_some_and(|s| s.rejected.is_some())
    );
}

#[tokio::test]
async fn an_unreachable_backend_is_exit_5_with_empty_stdout() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/auth/me"))
        .respond_with(ResponseTemplate::new(503).set_body_json(error_body(
            "iam_unavailable",
            "IAM is down",
            true,
        )))
        .mount(&server)
        .await;
    let env = Env::new(&server.uri());
    env.login_as("oat_ok", "ort_ok", unix_now() + 1800);
    let run = env.run(&["login", "status", "--json"]).await;
    assert_eq!(run.code, 5, "{}", run.stderr);
    assert!(
        run.stdout.is_empty(),
        "stdout must be empty: {}",
        run.stdout
    );
    let e = run.error();
    assert_eq!(e["code"], "iam_unavailable");
    assert_eq!(e["retryable"], true);

    let dead = Env::new(DEAD_API);
    dead.login_as("oat_ok", "ort_ok", unix_now() + 1800);
    let run = dead.run(&["login", "status", "--json"]).await;
    assert_eq!(run.code, 5);
    assert!(run.stdout.is_empty());
    assert_eq!(run.error()["code"], "backend_unavailable");
}

#[tokio::test]
async fn testing_environments_are_discovered_saved_and_bannered() {
    let server = MockServer::start().await;
    let env_id = "01927d6e-1c7a-7cc3-9d2e-4b4c1c1f0a11";
    let secret = format!("ask_{}", "t".repeat(43));
    Mock::given(method("GET"))
        .and(path("/api/v1/iam"))
        .and(header("x-testing-environment-key", secret.as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "app_id":"peek","api_version":"v1","api_base_url":server.uri(),
            "iam_base_url":"https://backend.iam.teamofsilicons.com",
            "testing_environment_id":env_id,"testing_generation":3,
            "testing_environment":{"id":env_id,"name":"peek testing","generation":3},
            "compatibility":{"cli":">=0.1.0, <1.0.0","ipc_protocols":[1]}
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login"))
        .and(header("x-testing-environment-key", secret.as_str()))
        .and(body_string(r#"{"slt":"si:peek-tester"}"#))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(session_body("oat_t", "ort_t", 1800)),
        )
        .expect(1)
        .mount(&server)
        .await;
    let env = Env::new(&server.uri());
    let run = env
        .run_stdin(
            &[
                "--app-secret-file",
                "-",
                "login",
                "si:peek-tester",
                "--json",
            ],
            Some(format!("{secret}\n").as_bytes()),
        )
        .await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.json()["testing_environment_id"], env_id);
    assert_eq!(
        run.stderr.lines().last(),
        Some(format!("Testing environment: peek testing ({env_id})").as_str())
    );
    let testing = env.store().read_testing().unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(testing.environments.len(), 1);
    let key = format!("{}#{env_id}", server.uri());
    assert!(env.session().slots.contains_key(&key), "slot {key}");
    assert!(
        !env.session()
            .slots
            .contains_key(&env.slot_key().as_string()),
        "no production slot"
    );

    // Later runs select it by id; the banner is the last stderr line even on failure.
    let iam = env.run(&["--test", env_id, "iam", "--json"]).await;
    assert_eq!(iam.code, 0, "{}", iam.stderr);
    assert_eq!(
        iam.json()["testing"],
        json!({"environment_id": env_id, "name": "peek testing", "generation": 3})
    );
    let report = env
        .run(&["--test", env_id, "report", "a bug", "--json"])
        .await;
    assert_eq!(report.code, 2);
    assert!(
        report
            .stderr
            .trim_end()
            .ends_with(&format!("Testing environment: peek testing ({env_id})"))
    );
    let secret_as_test = env
        .run(&["--test", &secret, "login", "status", "--json"])
        .await;
    assert_eq!(secret_as_test.code, 3);
    assert_eq!(secret_as_test.error()["code"], "testing_secret_invalid");
}

#[tokio::test]
async fn ting_enroll_records_the_subscription() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/ting/recipient"))
        .and(header("authorization", "Bearer oat_e"))
        .and(header("x-org-id", "tos"))
        .and(header("idempotency-key", "peek-enroll-0000000001"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"subscribed":true,"subscription_id":"sub_9"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let env = Env::new(&server.uri());
    env.login_as("oat_e", "ort_e", unix_now() + 1800);
    let run = env
        .run(&[
            "ting",
            "enroll",
            "--json",
            "--idempotency-key",
            "peek-enroll-0000000001",
        ])
        .await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(
        run.json(),
        json!({"subscribed": true, "subscription_id": "sub_9"})
    );
    let slot = env
        .session()
        .slot(&env.slot_key())
        .cloned()
        .unwrap_or_else(|| panic!("slot"));
    assert_eq!(
        slot.ting.and_then(|t| t.subscription_id).as_deref(),
        Some("sub_9")
    );
}

#[tokio::test]
async fn a_changed_testing_generation_is_rediscovered_and_retried_once() {
    let server = MockServer::start().await;
    let env_id = "01927d6e-1c7a-7cc3-9d2e-4b4c1c1f0a11";
    let secret = format!("ask_{}", "g".repeat(43));
    let discovery = |generation: u64| {
        json!({
            "app_id":"peek","api_version":"v1","api_base_url":server.uri(),
            "iam_base_url":"https://backend.iam.teamofsilicons.com",
            "testing_environment_id":env_id,"testing_generation":generation,
            "testing_environment":{"id":env_id,"name":"peek testing","generation":generation},
            "compatibility":{"cli":">=0.1.0, <1.0.0","ipc_protocols":[1]}
        })
    };
    // Login discovers generation 3; the environment is then cleaned (4).
    Mock::given(method("GET"))
        .and(path("/api/v1/iam"))
        .respond_with(ResponseTemplate::new(200).set_body_json(discovery(3)))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/iam"))
        .and(header("x-testing-environment-key", secret.as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(discovery(4)))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(session_body("oat_g", "ort_g", 1800)),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/ting/recipient"))
        .and(header("x-testing-environment-generation", "3"))
        .respond_with(ResponseTemplate::new(409).set_body_json(json!({"error":{
            "code":"testing_generation_changed","message":"the environment moved to generation 4",
            "hint":null,"retryable":false,"request_id":"r1","details":{"generation":4}}})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/ting/recipient"))
        .and(header("x-testing-environment-generation", "4"))
        .and(header("x-testing-environment-key", secret.as_str()))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"subscribed":true,"subscription_id":"sub_g"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let env = Env::new(&server.uri());
    let login = env
        .run_stdin(
            &[
                "--app-secret-file",
                "-",
                "login",
                "si:peek-tester",
                "--json",
            ],
            Some(format!("{secret}\n").as_bytes()),
        )
        .await;
    assert_eq!(login.code, 0, "{}", login.stderr);
    let run = env
        .run(&["--test", env_id, "ting", "enroll", "--json"])
        .await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.json()["subscription_id"], "sub_g");
    let testing = env.store().read_testing().unwrap_or_else(|e| panic!("{e}"));
    let saved = testing
        .environments
        .values()
        .next()
        .unwrap_or_else(|| panic!("saved environment"));
    assert_eq!(saved.generation, 4, "the new generation is saved");
    server.verify().await;
}

#[tokio::test]
async fn idempotency_key_is_refused_where_keys_are_derived() {
    let env = Env::new(DEAD_API);
    let run = env
        .run(&[
            "login",
            "oac_x",
            "--idempotency-key",
            "abcdefghijklmnop",
            "--json",
        ])
        .await;
    assert_eq!(run.code, 2);
    assert_eq!(run.error()["code"], "conflicting_flags");
}

#[tokio::test]
async fn report_goes_through_the_backend() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/reports"))
        .and(header_exists("idempotency-key"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id":"rep_1","status":"stored","issue_url":null})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let env = Env::new(&server.uri());
    let run = env.run(&["report", "send fails\nsteps: …", "--json"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(
        run.json(),
        json!({"submitted": true, "id": "rep_1", "status": "stored", "issue_url": null, "via": "backend"})
    );
    let requests = server.received_requests().await.unwrap_or_default();
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap_or_default();
    assert_eq!(body["message"], "send fails\nsteps: …");
    assert_eq!(body["context"]["cli_version"], env!("CARGO_PKG_VERSION"));
    let bad = env
        .run(&[
            "report",
            "x",
            "--pr",
            "https://github.com/other/repo/pull/1",
            "--json",
        ])
        .await;
    assert_eq!(bad.code, 2);
    assert_eq!(bad.error()["code"], "invalid_input");
}

#[tokio::test]
async fn report_dry_run_prints_the_payload_and_sends_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/reports"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let env = Env::new(&server.uri());
    let run = env
        .run(&[
            "report",
            "dogfood check\nplease ignore",
            "--dry-run",
            "--json",
        ])
        .await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    let v = run.json();
    assert_eq!(v["dry_run"], true);
    assert_eq!(v["via"], "backend");
    assert_eq!(v["would_send"]["method"], "POST");
    assert_eq!(
        v["would_send"]["url"],
        format!("{}/api/v1/reports", server.uri())
    );
    assert_eq!(
        v["would_send"]["body"]["message"],
        "dogfood check\nplease ignore"
    );
    let gh = env
        .run(&[
            "report",
            "a title\nbody",
            "--via",
            "gh",
            "--dry-run",
            "--json",
        ])
        .await;
    assert_eq!(gh.code, 0, "{}", gh.stderr);
    assert_eq!(gh.json()["would_send"]["title"], "a title");
}

#[tokio::test]
async fn byo_deepgram_show_set_and_refusal() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/orgs/tos/byo/deepgram"))
        .and(header("authorization", "Bearer oat_b"))
        .and(header("x-org-id", "tos"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"configured":false,"updated_at":null,"base_url":null})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/orgs/tos/byo/deepgram"))
        .and(body_string(
            r#"{"api_key":"dg_secret_key","base_url":"https://api.eu.deepgram.com"}"#,
        ))
        .respond_with(ResponseTemplate::new(403).set_body_json(error_body(
            "not_org_admin",
            "si:cleanup is not an owner or admin of tos",
            false,
        )))
        .expect(1)
        .mount(&server)
        .await;
    let env = Env::new(&server.uri());
    env.login_as("oat_b", "ort_b", unix_now() + 1800);
    let show = env.run(&["org", "byo", "deepgram", "show", "--json"]).await;
    assert_eq!(show.code, 0, "{}", show.stderr);
    assert_eq!(
        show.json(),
        json!({"configured": false, "updated_at": null, "base_url": null, "org_id": "tos"})
    );
    let set = env
        .run_stdin(
            &[
                "org",
                "byo",
                "deepgram",
                "set",
                "--key-file",
                "-",
                "--base-url",
                "https://api.eu.deepgram.com/",
                "--json",
            ],
            Some(b"dg_secret_key\n"),
        )
        .await;
    assert_eq!(set.code, 4, "{}", set.stderr);
    assert_eq!(set.error()["code"], "not_org_admin");
    assert!(
        !set.stderr.contains("dg_secret_key"),
        "the key is never echoed"
    );
    let other_org = env
        .run(&["--org", "acme", "org", "byo", "deepgram", "show", "--json"])
        .await;
    assert_eq!(other_org.code, 2);
    assert_eq!(other_org.error()["details"]["session_orgs"], json!(["tos"]));
}
