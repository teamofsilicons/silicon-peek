//! Real CLI context isolation across accounts, orgs and legacy credentials.
#![allow(clippy::unwrap_used)]
mod common;
use common::{Env, session_body};
use serde_json::json;
use silicon_peek_client::runtime::Store;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_string, method, path},
};

#[tokio::test]
async fn named_profiles_preserve_independent_carbon_and_silicon_logins() {
    let server = MockServer::start().await;
    let env = Env::new(&server.uri());
    let silicon = session_body("oat_work", "ort_work", 3600);
    let mut carbon = session_body("oat_home", "ort_home", 3600);
    carbon["actor"] = json!({"type":"carbon","public_id":"c:alice"});
    carbon["org_id"] = json!("personal");
    carbon["org_ids"] = json!(["personal"]);
    carbon["membership_id"] = json!("c:alice[personal]");
    for (token, body) in [("oac_work", silicon), ("oac_home", carbon)] {
        Mock::given(method("POST"))
            .and(path("/api/v1/auth/login"))
            .and(body_string(format!(r#"{{"slt":"{token}"}}"#)))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .expect(1)
            .mount(&server)
            .await;
    }
    for (profile, token) in [("work", "oac_work"), ("personal", "oac_home")] {
        let run = env
            .run(&["--profile", profile, "login", token, "--json"])
            .await;
        assert_eq!(run.code, 0, "{}", run.stderr);
    }
    let work = Store::open_existing(&env.store_dir().join("profiles/work"))
        .unwrap()
        .read_session()
        .unwrap();
    let home = Store::open_existing(&env.store_dir().join("profiles/personal"))
        .unwrap()
        .read_session()
        .unwrap();
    let work = work.slot(&env.slot_key()).unwrap();
    let home = home.slot(&env.slot_key()).unwrap();
    assert_ne!(work.context_id().unwrap(), home.context_id().unwrap());
    assert_eq!(work.refresh_token.expose(), "ort_work");
    assert_eq!(home.refresh_token.expose(), "ort_home");
    assert!(!env.store_dir().join("session.json").exists());
    let mismatch = env
        .run(&[
            "--profile",
            "work",
            "--org",
            "personal",
            "org",
            "byo",
            "deepgram",
            "show",
            "--json",
        ])
        .await;
    assert_ne!(mismatch.code, 0);
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
    let invalid = env
        .run(&["--profile", "../escape", "login", "status", "--json"])
        .await;
    assert_eq!(invalid.code, 2);
}

#[tokio::test]
async fn legacy_session_needs_reauthentication_before_network() {
    let server = MockServer::start().await;
    let env = Env::new(&server.uri());
    env.login_as("oat_legacy", "ort_legacy", 1);
    env.store()
        .update_session(|file| {
            file.slots
                .get_mut(&env.slot_key().as_string())
                .unwrap()
                .extra
                .remove("context_id");
            Ok(())
        })
        .unwrap();
    let run = env.run(&["org", "byo", "deepgram", "show", "--json"]).await;
    assert_ne!(run.code, 0);
    assert_eq!(run.error()["code"], "session_rejected");
    assert!(server.received_requests().await.unwrap().is_empty());
}
