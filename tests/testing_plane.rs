//! Testing environments (BLUEPRINT §2.9): the Honeycomb lifecycle participant
//! (receipts, idempotency, barrier → wipe → receipt), plane resolution from
//! the validated peek test secret only, generations, data isolation, and the
//! Ting test headers taken from the proof.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use axum::{body::Body, http::Request};
use common::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use silicon_peek::state::Limits;
use wiremock::{
    Mock, ResponseTemplate,
    matchers::{header, method, path},
};

const SCRIPT: &str = "export default function draw() {}";
const TEST_ACCESS: &str = "oat_testPlaneAccess01";
const BOGUS_SECRET: &str = "ask_unknownunknownunknownunknownunknownunknownu";
const OTHER_TING_SECRET: &str = "ask_someOtherTingSecretsomeOtherTingSecretsomeO";

fn discover() -> Request<Body> {
    testing(Request::get("/api/v1/iam"), None)
        .body(Body::empty())
        .unwrap()
}

/// Which plane a request targets.
enum On {
    Production,
    /// The testing plane, with this generation header (if any).
    Testing(Option<u64>),
}

fn put_drawing(token: &str, on: &On) -> Request<Body> {
    let mut b = authed("PUT", "/api/v1/drawings/current", token)
        .header("content-type", "application/javascript")
        .header(
            "x-peek-drawing-sha256",
            hex::encode(Sha256::digest(SCRIPT.as_bytes())),
        );
    if let On::Testing(generation) = on {
        b = testing(b, *generation);
    }
    b.body(Body::from(SCRIPT)).unwrap()
}

fn get_drawing(token: &str, testing_plane: bool) -> Request<Body> {
    let mut b = authed("GET", "/api/v1/drawings/current", token);
    if testing_plane {
        b = testing(b, None);
    }
    b.body(Body::empty()).unwrap()
}

async fn testing_harness() -> Harness {
    let h = Harness::start().await;
    mount_testing_contexts(&h.iam).await;
    mount_silicon(&h.iam, ACCESS).await;
    mount_introspect_testing(
        &h.iam,
        TEST_ACCESS,
        introspection(ACTOR, ORG, &FULL_SCOPES, Some("admin"), Some(env_id())),
    )
    .await;
    // IAM's testing plane does not know production tokens.
    mount_introspect_testing(&h.iam, ACCESS, json!({"active": false})).await;
    h
}

#[tokio::test]
async fn the_participant_authenticates_honeycomb() {
    let h = Harness::start().await;
    let op = operation("prepare", 1, 1, 1, ROOT_KEY);
    let mut req = participant_request(&op);
    req.headers_mut().remove("authorization");
    assert_eq!(h.send(req).await.status, 401);
    let mut req = participant_request(&op);
    req.headers_mut().insert(
        "authorization",
        "Bearer not-the-service-token-at-all-000".parse().unwrap(),
    );
    let r = h.send(req).await;
    assert_eq!(r.status, 401);
    assert_eq!(r.code(), "unauthenticated");

    let unconfigured = Harness::with(
        |env| {
            env.remove("PEEK_HONEYCOMB_SERVICE_TOKEN");
        },
        Limits::default(),
    )
    .await;
    assert_eq!(
        unconfigured.send(participant_request(&op)).await.status,
        503
    );
}

#[tokio::test]
async fn receipts_are_durable_and_idempotent() {
    let h = Harness::start().await;
    let op = operation("prepare", 1, 1, 1, ROOT_KEY);
    let r = h.send(participant_request(&op)).await;
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let receipt = r.json();
    assert_eq!(
        receipt,
        json!({"state": "completed", "operation_id": op["operation_id"], "environment_id": op["environment_id"],
               "app_id": "peek", "environment_revision": 1, "generation": 1, "key_version": 1})
    );
    assert!(
        !String::from_utf8_lossy(&r.body).contains(ROOT_KEY),
        "receipts never contain keys"
    );

    let replay = h.send(participant_request(&op)).await;
    assert_eq!(
        replay.json(),
        receipt,
        "a replay returns the stored receipt"
    );
    let get = h
        .send(
            Request::get(format!(
                "/internal/honeycomb/organizations/tos/testing-environments/{}/operations/{}",
                op["environment_id"].as_str().unwrap(),
                op["operation_id"].as_str().unwrap()
            ))
            .header("authorization", format!("Bearer {SERVICE_TOKEN}"))
            .body(Body::empty())
            .unwrap(),
        )
        .await;
    assert_eq!(get.status, 200);
    assert_eq!(get.json(), receipt);

    let mut altered = op.clone();
    altered["reason"] = json!("different");
    let r = h.send(participant_request(&altered)).await;
    assert_eq!(r.status, 409);
    assert_eq!(r.code(), "idempotency_conflict");

    let unknown = h
        .send(
            Request::get(format!(
                "/internal/honeycomb/organizations/tos/testing-environments/{}/operations/{}",
                op["environment_id"].as_str().unwrap(),
                uuid::Uuid::now_v7()
            ))
            .header("authorization", format!("Bearer {SERVICE_TOKEN}"))
            .body(Body::empty())
            .unwrap(),
        )
        .await;
    assert_eq!(unknown.status, 404);
}

#[tokio::test]
async fn instructions_are_validated() {
    let h = Harness::start().await;
    // Before prepare, nothing else is accepted.
    let r = h
        .send(participant_request(&operation("clean", 1, 2, 1, ROOT_KEY)))
        .await;
    assert_eq!(r.status, 409);

    let mut wrong_app = operation("prepare", 1, 1, 1, ROOT_KEY);
    wrong_app["app_id"] = json!("dm");
    assert_eq!(h.send(participant_request(&wrong_app)).await.status, 400);

    let mut unknown_field = operation("prepare", 1, 1, 1, ROOT_KEY);
    unknown_field["name"] = json!("x");
    assert_eq!(
        h.send(participant_request(&unknown_field)).await.status,
        400
    );

    let bad_key = operation("prepare", 1, 1, 1, "short");
    assert_eq!(h.send(participant_request(&bad_key)).await.status, 400);

    let op = operation("prepare", 1, 1, 1, ROOT_KEY);
    let mut req = participant_request(&op);
    *req.uri_mut() = format!(
        "/internal/honeycomb/organizations/other/testing-environments/{}/operations/{}",
        op["environment_id"].as_str().unwrap(),
        op["operation_id"].as_str().unwrap()
    )
    .parse()
    .unwrap();
    assert_eq!(h.send(req).await.status, 400, "route and body must agree");

    h.prepare_environment().await;
    let stale = h
        .send(participant_request(&operation("import", 1, 1, 1, ROOT_KEY)))
        .await;
    assert_eq!(stale.status, 409, "revisions must advance");
    let same_key = h
        .send(participant_request(&operation(
            "rotate-key",
            2,
            1,
            2,
            ROOT_KEY,
        )))
        .await;
    assert_eq!(same_key.status, 409, "rotation must change the key");
    let changed = h
        .send(participant_request(&operation(
            "import",
            2,
            1,
            1,
            "Changed0123456789Changed01234567",
        )))
        .await;
    assert_eq!(changed.status, 409, "a new key needs a new key_version");
}

#[tokio::test]
async fn planes_resolve_only_from_a_validated_prepared_secret() {
    let h = testing_harness().await;
    let r = h.send(discover()).await;
    assert_eq!(r.status, 409);
    assert_eq!(r.code(), "environment_not_prepared");

    h.prepare_environment().await;
    let r = h.send(discover()).await;
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let v = r.json();
    assert_eq!(v["testing_environment_id"], env_id().to_string());
    assert_eq!(v["testing_generation"], 1);
    assert_eq!(
        v["testing_environment"],
        json!({"id": env_id(), "name": "peek testing", "generation": 1})
    );

    Mock::given(method("GET"))
        .and(path("/api/v1/application/testing-context"))
        .and(header(
            "x-testing-application",
            basic("peek", BOGUS_SECRET).as_str(),
        ))
        .respond_with(iam_error(401, "invalid_client"))
        .mount(&h.iam)
        .await;
    let bogus = h
        .send(
            Request::get("/api/v1/iam")
                .header("x-testing-environment-key", BOGUS_SECRET)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(bogus.status, 401);
    assert_eq!(bogus.code(), "testing_secret_invalid");

    let malformed = h
        .send(
            Request::get("/api/v1/iam")
                .header(
                    "x-testing-environment-key",
                    "0192f2d2-7c9e-7cc0-8b2e-6f3a2b1c0d9e",
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(
        malformed.status, 401,
        "a UUID is never accepted in place of the secret"
    );

    let orphan = h
        .send(
            Request::get("/api/v1/iam")
                .header("x-testing-environment-generation", "1")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(orphan.status, 400);
}

#[tokio::test]
async fn generations_fence_mutations_and_clean_wipes_the_plane() {
    let h = testing_harness().await;
    h.prepare_environment().await;

    let r = h.send(put_drawing(TEST_ACCESS, &On::Testing(None))).await;
    assert_eq!(r.status, 409, "a mutation must carry the generation");
    assert_eq!(r.code(), "testing_generation_changed");
    let r = h
        .send(put_drawing(TEST_ACCESS, &On::Testing(Some(7))))
        .await;
    assert_eq!(r.status, 409);
    assert_eq!(r.json()["error"]["details"]["generation"], 1);

    assert_eq!(
        h.send(put_drawing(TEST_ACCESS, &On::Testing(Some(1))))
            .await
            .status,
        200
    );
    assert_eq!(h.send(get_drawing(TEST_ACCESS, true)).await.status, 200);
    assert_eq!(
        h.send(get_drawing(ACCESS, false)).await.status,
        404,
        "production never sees test data"
    );
    assert_eq!(
        h.send(put_drawing(ACCESS, &On::Production)).await.status,
        200
    );

    // A production token presented with the testing key is not accepted.
    let r = h.send(get_drawing(ACCESS, true)).await;
    assert_eq!(r.status, 401);

    let clean = operation("clean", 2, 2, 1, ROOT_KEY);
    let r = h.send(participant_request(&clean)).await;
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.json()["generation"], 2);
    assert_eq!(
        h.send(get_drawing(TEST_ACCESS, true)).await.status,
        404,
        "clean wiped the plane"
    );
    assert_eq!(
        h.send(get_drawing(ACCESS, false)).await.status,
        200,
        "production survives"
    );
    assert_eq!(
        h.send(put_drawing(TEST_ACCESS, &On::Testing(Some(1))))
            .await
            .status,
        409,
        "old generation"
    );
    assert_eq!(h.send(discover()).await.json()["testing_generation"], 2);
}

#[tokio::test]
async fn disable_restore_and_purge() {
    let h = testing_harness().await;
    h.prepare_environment().await;
    assert_eq!(
        h.send(participant_request(&operation(
            "disable", 2, 1, 1, ROOT_KEY
        )))
        .await
        .status,
        200
    );
    let r = h.send(discover()).await;
    assert_eq!(r.status, 409);
    assert_eq!(r.code(), "environment_not_prepared");
    let r = h
        .send(participant_request(&operation("import", 3, 1, 1, ROOT_KEY)))
        .await;
    assert_eq!(
        r.status, 409,
        "a disabled environment only accepts restore/purge/disable/clean/rotate"
    );
    assert_eq!(
        h.send(participant_request(&operation(
            "restore", 3, 1, 1, ROOT_KEY
        )))
        .await
        .status,
        200
    );
    assert_eq!(h.send(discover()).await.status, 200);

    let mut purge = operation("purge", 4, 1, 1, ROOT_KEY);
    purge.as_object_mut().unwrap().remove("testing_key");
    assert_eq!(h.send(participant_request(&purge)).await.status, 200);
    let r = h.send(discover()).await;
    assert_eq!(r.status, 401, "a purged environment is gone");
    let r = h
        .send(participant_request(&operation(
            "prepare", 5, 1, 1, ROOT_KEY,
        )))
        .await;
    assert_eq!(r.status, 409, "purge is irreversible");
}

#[tokio::test]
async fn test_login_uses_the_test_secret_and_forwards_tings_test_headers() {
    let h = testing_harness().await;
    h.prepare_environment().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/app-auth/tokens"))
        .and(header("authorization", basic("peek", TEST_SECRET).as_str()))
        .and(header(
            "x-testing-application",
            basic("peek", TEST_SECRET).as_str(),
        ))
        .and(wiremock::matchers::body_string_contains("slt=si%3Acleanup"))
        .respond_with(ResponseTemplate::new(200).set_body_json(token_response(
            TEST_ACCESS,
            "ort_testPlaneRefresh01",
            &FULL_SCOPES,
        )))
        .mount(&h.iam)
        .await;
    mount_authorizations(
        &h.iam,
        TEST_ACCESS,
        json!([authorization(
            ACTOR,
            ORG,
            &FULL_SCOPES,
            None,
            Some(env_id())
        )]),
    )
    .await;
    mount_catalog(&h.iam).await;
    mount_exchange_for(
        &h.iam,
        TEST_SECRET,
        Some(json!({"app_id": "ting", "app_secret": TING_TEST_SECRET, "iam_test_key": ROOT_KEY})),
    )
    .await;
    mount_ting_register(&h.ting).await;

    let r = h
        .send(json_body(
            testing(post("/api/v1/auth/login"), None),
            &json!({"slt": ACTOR}),
        ))
        .await;
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let v: Value = r.json();
    assert_eq!(v["access_token"], TEST_ACCESS);
    assert_eq!(
        v["testing_environment"],
        json!({"id": env_id(), "name": "peek testing", "generation": 1})
    );
    assert_eq!(v["ting"]["subscribed"], true);
    let regs = Harness::requests(&h.ting, "/v1/subscriptions").await;
    assert_eq!(regs.len(), 1);
    assert_eq!(
        regs[0].headers["iam_test_app_secret"], TING_TEST_SECRET,
        "Ting's audience secret, from the proof"
    );
    assert_eq!(
        regs[0].headers["x-testing-environment-key"], ROOT_KEY,
        "the root key, never peek's secret"
    );
    assert!(
        !regs[0]
            .headers
            .values()
            .any(|v| v.to_str().is_ok_and(|s| s.contains(TEST_SECRET))),
        "peek's test secret never reaches Ting"
    );
}

#[tokio::test]
async fn a_proof_for_another_environment_is_refused() {
    let h = testing_harness().await;
    h.prepare_environment().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/application/testing-context"))
        .and(header(
            "authorization",
            basic("ting", OTHER_TING_SECRET).as_str(),
        ))
        .respond_with(iam_error(401, "invalid_client"))
        .mount(&h.iam)
        .await;
    mount_catalog(&h.iam).await;
    mount_exchange_for(
        &h.iam,
        TEST_SECRET,
        Some(json!({"app_id": "ting", "app_secret": OTHER_TING_SECRET, "iam_test_key": ROOT_KEY})),
    )
    .await;
    mount_ting_register(&h.ting).await;
    let r = h
        .send(json_body(
            testing(
                authed("POST", "/api/v1/ting/recipient", TEST_ACCESS)
                    .header("content-type", "application/json")
                    .header("idempotency-key", IDEM),
                Some(1),
            ),
            &json!({}),
        ))
        .await;
    assert_eq!(r.status, 502, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.code(), "ting_rejected");
    assert!(
        Harness::requests(&h.ting, "/v1/subscriptions")
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn activity_is_reported_with_the_root_key() {
    let h = testing_harness().await;
    h.prepare_environment().await;
    Mock::given(method("POST"))
        .and(path(format!(
            "/api/v1/environments/{}/apps/peek/activity",
            env_id()
        )))
        .and(header("x-testing-environment-key", ROOT_KEY))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&h.honeycomb)
        .await;
    assert_eq!(h.send(discover()).await.status, 200);
    h.state.maintenance().await;
    h.state.maintenance().await;
    let reports = Harness::requests(
        &h.honeycomb,
        &format!("/api/v1/environments/{}/apps/peek/activity", env_id()),
    )
    .await;
    assert_eq!(reports.len(), 1, "reported once, then acknowledged");
    let body: Value = serde_json::from_slice(&reports[0].body).unwrap();
    assert_eq!(body, json!({"generation": 1, "key_version": 1}));
    assert!(reports[0].headers.contains_key("idempotency-key"));
    assert!(
        h.events()
            .iter()
            .all(|e| e["context"]["http_route"] != "/api/v1/iam"),
        "test traffic records no telemetry"
    );
}
