//! Login, refresh, logout and `/me` against a fake IAM (BLUEPRINT §2.4,
//! §2.6, §2.7): the exact IAM calls, the error mapping, the Ting enrollment
//! at login, and no token ever persisted.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use axum::{body::Body, http::Request};
use common::*;
use serde_json::{Value, json};
use wiremock::{
    Mock, ResponseTemplate,
    matchers::{method, path},
};

const SLT: &str = "oac_abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG";

async fn mount_login_happy_path(h: &Harness, scopes: &[&str]) {
    mount_token_exchange(
        &h.iam,
        &format!("slt={SLT}"),
        ResponseTemplate::new(200).set_body_json(token_response(ACCESS, REFRESH, scopes)),
    )
    .await;
    mount_authorizations(
        &h.iam,
        ACCESS,
        json!([authorization(ACTOR, ORG, scopes, Some("member"), None)]),
    )
    .await;
    mount_me(&h.iam, "Cleanup").await;
    mount_catalog(&h.iam).await;
    mount_exchange(&h.iam, None).await;
    mount_ting_register(&h.ting).await;
    mount_silicon(&h.iam, ACCESS).await;
}

fn login_request(key: &str) -> Request<Body> {
    Request::post("/api/v1/auth/login")
        .header("content-type", "application/json")
        .header("idempotency-key", key)
        .body(Body::from(json!({"slt": SLT}).to_string()))
        .unwrap()
}

#[tokio::test]
async fn login_exchanges_verifies_enrolls_and_returns_the_session() {
    let h = Harness::start().await;
    mount_login_happy_path(&h, &FULL_SCOPES).await;
    let key = "peek-login-0123456789abcdef";
    let r = h.send(login_request(key)).await;
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.header("cache-control").as_deref(), Some("no-store"));
    assert_eq!(r.header("pragma").as_deref(), Some("no-cache"));
    let v = r.json();
    assert_eq!(v["access_token"], ACCESS);
    assert_eq!(v["refresh_token"], REFRESH);
    assert_eq!(v["token_type"], "Bearer");
    assert_eq!(v["expires_in"], 1800);
    assert_eq!(
        v["scope"],
        "obo:ting:subscriptions.register obo:ting:subscriptions.revoke obo:ting:tings.send self.identity.read self.membership.read self.profile.read"
    );
    assert_eq!(v["actor"], json!({"type": "silicon", "public_id": ACTOR}));
    assert_eq!(v["org_id"], ORG);
    assert_eq!(v["org_ids"], json!([ORG]));
    assert_eq!(v["membership_id"], "si:cleanup[tos]");
    assert_eq!(v["reconsent_required"], false);
    assert_eq!(v["display_name"], "Cleanup");
    assert_eq!(
        v["ting"],
        json!({"subscribed": true, "subscription_id": "sub_1"})
    );
    assert!(v["testing_environment"].is_null());

    // The exact IAM exchange: Basic peek:<secret>, the CLI's key, form app_id+slt.
    let exchanges = Harness::requests(&h.iam, "/api/v1/app-auth/tokens").await;
    assert_eq!(exchanges.len(), 1);
    let x = &exchanges[0];
    assert_eq!(
        x.headers["authorization"],
        basic("peek", APP_SECRET).as_str()
    );
    assert_eq!(x.headers["idempotency-key"], key);
    let form = String::from_utf8_lossy(&x.body);
    assert!(
        form.contains("app_id=peek") && form.contains(&format!("slt={SLT}")),
        "{form}"
    );

    // Enrollment: exactly {"app_id":"peek","for":…,"org_id":…} with a proof.
    let regs = Harness::requests(&h.ting, "/v1/subscriptions").await;
    assert_eq!(regs.len(), 1);
    assert_eq!(
        regs[0].body,
        br#"{"app_id":"peek","for":"si:cleanup","org_id":"tos"}"#
    );
    assert_eq!(regs[0].headers["authorization"], "Bearer proof-1");
    assert!(!regs[0].headers.contains_key("iam_test_app_secret"));
    let exchange: Value = serde_json::from_slice(
        &Harness::requests(&h.iam, "/api/v1/obo-access/exchanges").await[0].body,
    )
    .unwrap();
    assert_eq!(exchange["endpoint_id"], "subscriptions.register");
    assert_eq!(exchange["audience"], "ting");
    assert_eq!(exchange["subject_token"], ACCESS);
    assert_eq!(exchange["org_id"], ORG);
    assert_eq!(exchange["metadata"], json!({}));
    assert_eq!(
        exchange["request"]["body_sha256"],
        silicon_iam_client::api::obo::body_sha256(&regs[0].body)
    );

    // /me now reports the enrollment; the backend stored no token anywhere.
    let me = h
        .send(
            authed("GET", "/api/v1/auth/me", ACCESS)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(me.status, 200);
    assert_eq!(
        me.json()["ting"],
        json!({"subscribed": true, "subscription_id": "sub_1"})
    );
    for file in ["peek.sqlite", "testing.sqlite"] {
        let bytes = std::fs::read(h.dir.path().join(file)).unwrap();
        let wal = std::fs::read(h.dir.path().join(format!("{file}-wal"))).unwrap_or_default();
        for haystack in [&bytes, &wal] {
            let text = String::from_utf8_lossy(haystack);
            assert!(
                !text.contains(ACCESS) && !text.contains(REFRESH),
                "no token persisted in {file}"
            );
        }
    }
    let events = h.events();
    let login = events.iter().find(|e| e["event"] == "auth.login").unwrap();
    assert_eq!(login["outcome"], "ok");
    assert!(!login.to_string().contains(ACTOR), "actor IDs are hashed");
}

#[tokio::test]
async fn login_prefers_the_org_hint_among_grants() {
    let h = Harness::start().await;
    mount_token_exchange(
        &h.iam,
        &format!("slt={SLT}"),
        ResponseTemplate::new(200).set_body_json(token_response(ACCESS, REFRESH, &FULL_SCOPES)),
    )
    .await;
    mount_authorizations(
        &h.iam,
        ACCESS,
        json!([
            authorization(ACTOR, "zeta", &FULL_SCOPES, None, None),
            authorization(ACTOR, "acme", &FULL_SCOPES, None, None)
        ]),
    )
    .await;
    mount_catalog(&h.iam).await;
    mount_exchange(&h.iam, None).await;
    mount_ting_register(&h.ting).await;
    let mut req = login_request(IDEM);
    req.headers_mut()
        .insert("x-org-id", "zeta".parse().unwrap());
    let r = h.send(req).await;
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.json()["org_id"], "zeta");
    assert_eq!(r.json()["org_ids"], json!(["acme", "zeta"]));

    let r = h.send(login_request("peek-login-without-hint-01")).await;
    assert_eq!(
        r.json()["org_id"],
        "acme",
        "without a hint, the first org sorted"
    );
}

#[tokio::test]
async fn login_without_ting_scopes_is_reconsent_and_skips_enrollment() {
    let h = Harness::start().await;
    let scopes = ["self.identity.read", "self.profile.read"];
    mount_login_happy_path(&h, &scopes).await;
    let r = h.send(login_request(IDEM)).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json()["reconsent_required"], true);
    assert_eq!(r.json()["ting"]["subscribed"], false);
    assert_eq!(r.json()["ting"]["error"]["code"], "reconsent_required");
    assert!(
        Harness::requests(&h.ting, "/v1/subscriptions")
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn a_failed_enrollment_never_fails_the_login() {
    let h = Harness::start().await;
    mount_token_exchange(
        &h.iam,
        &format!("slt={SLT}"),
        ResponseTemplate::new(200).set_body_json(token_response(ACCESS, REFRESH, &FULL_SCOPES)),
    )
    .await;
    mount_authorizations(
        &h.iam,
        ACCESS,
        json!([authorization(ACTOR, ORG, &FULL_SCOPES, None, None)]),
    )
    .await;
    mount_catalog(&h.iam).await;
    mount_exchange(&h.iam, None).await;
    Mock::given(method("POST"))
        .and(path("/v1/subscriptions"))
        .respond_with(ting_error(503, "dependency_unavailable"))
        .mount(&h.ting)
        .await;
    let r = h.send(login_request(IDEM)).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json()["ting"]["subscribed"], false);
    assert_eq!(r.json()["ting"]["error"]["code"], "ting_unavailable");
    assert_eq!(r.json()["access_token"], ACCESS);
}

#[tokio::test]
async fn login_error_mapping() {
    for (status, code, expected_status, expected_code) in [
        (400, "invalid_grant", 401, "slt_rejected"),
        (401, "unauthenticated", 401, "slt_rejected"),
        (410, "slt_expired", 401, "slt_rejected"),
        (
            403,
            "private_application_organization_required",
            403,
            "private_application_organization_required",
        ),
        (401, "invalid_client", 503, "iam_misconfigured"),
        (503, "unavailable", 503, "iam_unavailable"),
        (
            409,
            "idempotency_in_progress",
            409,
            "idempotency_in_progress",
        ),
    ] {
        let h = Harness::start().await;
        mount_token_exchange(&h.iam, "slt=", iam_error(status, code)).await;
        let r = h.send(login_request(IDEM)).await;
        assert_eq!(r.status, expected_status, "{code}");
        assert_eq!(r.code(), expected_code, "{code}");
        assert!(r.json()["error"]["hint"].is_string(), "{code} has a hint");
    }
}

#[tokio::test]
async fn login_refuses_public_ids_in_production_and_reports_transport_failures() {
    let h = Harness::start().await;
    let r = h
        .send(json_body(
            post("/api/v1/auth/login"),
            &json!({"slt": "si:cleanup"}),
        ))
        .await;
    assert_eq!(r.status, 400);
    assert_eq!(r.code(), "slt_is_public_id");

    let dead = Harness::with(
        |env| {
            env.insert("PEEK_IAM_BASE_URL", "http://127.0.0.1:9".to_owned());
        },
        silicon_peek::state::Limits::default(),
    )
    .await;
    let r = dead.send(login_request(IDEM)).await;
    assert_eq!(r.status, 503);
    assert_eq!(r.code(), "iam_unavailable");
    assert_eq!(r.json()["error"]["retryable"], true);
}

#[tokio::test]
async fn refresh_rotates_and_maps_terminal_errors() {
    let h = Harness::start().await;
    mount_token_exchange(
        &h.iam,
        &format!("refresh_token={REFRESH}"),
        ResponseTemplate::new(200).set_body_json(token_response(
            "oat_rotatedAccess01",
            "ort_rotatedRefresh01",
            &FULL_SCOPES,
        )),
    )
    .await;
    mount_authorizations(
        &h.iam,
        "oat_rotatedAccess01",
        json!([authorization(ACTOR, ORG, &FULL_SCOPES, None, None)]),
    )
    .await;
    let r = h
        .send(json_body(
            post("/api/v1/auth/refresh"),
            &json!({"refresh_token": REFRESH}),
        ))
        .await;
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.json()["access_token"], "oat_rotatedAccess01");
    assert_eq!(r.json()["refresh_token"], "ort_rotatedRefresh01");
    assert!(r.json().get("ting").is_none(), "refresh carries no ting");
    let x = &Harness::requests(&h.iam, "/api/v1/app-auth/tokens").await[0];
    assert_eq!(x.headers["idempotency-key"], IDEM);
    assert!(String::from_utf8_lossy(&x.body).contains("refresh_token=ort_cleanupRefreshToken01"));

    for (status, code, expected_status, expected_code) in [
        (400, "invalid_grant", 401, "session_rejected"),
        (400, "refresh_token_reuse", 401, "session_rejected"),
        (401, "unauthenticated", 401, "session_rejected"),
        (
            409,
            "idempotency_response_expired",
            409,
            "idempotency_response_expired",
        ),
        (401, "invalid_client", 503, "iam_misconfigured"),
        (502, "bad_gateway", 503, "iam_unavailable"),
    ] {
        let h = Harness::start().await;
        mount_token_exchange(&h.iam, "refresh_token=", iam_error(status, code)).await;
        let r = h
            .send(json_body(
                post("/api/v1/auth/refresh"),
                &json!({"refresh_token": REFRESH}),
            ))
            .await;
        assert_eq!(r.status, expected_status, "{code}");
        assert_eq!(r.code(), expected_code, "{code}");
    }

    let r = h
        .send(json_body(
            post("/api/v1/auth/refresh"),
            &json!({"refresh_token": "oat_notARefresh"}),
        ))
        .await;
    assert_eq!(r.code(), "session_rejected");
}

#[tokio::test]
async fn logout_revokes_the_ting_grant_then_the_family() {
    let h = Harness::start().await;
    mount_login_happy_path(&h, &FULL_SCOPES).await;
    assert_eq!(h.send(login_request(IDEM)).await.status, 200);
    Mock::given(method("POST"))
        .and(path("/v1/subscriptions/revoke"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id": "sub_1", "active": false})),
        )
        .mount(&h.ting)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/oauth/revoke"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&h.iam)
        .await;
    let r = h
        .send(json_body(
            authed("POST", "/api/v1/auth/logout", ACCESS)
                .header("content-type", "application/json")
                .header("idempotency-key", "peek-revoke-0123456789"),
            &json!({"token": REFRESH}),
        ))
        .await;
    assert_eq!(r.status, 204, "{}", String::from_utf8_lossy(&r.body));
    let revokes = Harness::requests(&h.ting, "/v1/subscriptions/revoke").await;
    assert_eq!(revokes.len(), 1);
    assert_eq!(revokes[0].body, br#"{"id":"sub_1","org_id":"tos"}"#);
    let iam_revoke = &Harness::requests(&h.iam, "/api/v1/oauth/revoke").await[0];
    let form = String::from_utf8_lossy(&iam_revoke.body);
    assert!(
        form.contains("token=ort_cleanupRefreshToken01")
            && form.contains("token_type_hint=refresh_token"),
        "{form}"
    );
    assert_eq!(
        iam_revoke.headers["idempotency-key"],
        "peek-revoke-0123456789"
    );

    let me = h
        .send(
            authed("GET", "/api/v1/auth/me", ACCESS)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(
        me.json()["ting"]["subscribed"],
        false,
        "the grant is recorded revoked"
    );
}

#[tokio::test]
async fn logout_of_an_unknown_token_succeeds_and_outages_are_retryable() {
    let h = Harness::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/oauth/revoke"))
        .respond_with(iam_error(400, "invalid_token"))
        .mount(&h.iam)
        .await;
    let r = h
        .send(json_body(
            post("/api/v1/auth/logout"),
            &json!({"token": "ort_unknownToken01"}),
        ))
        .await;
    assert_eq!(r.status, 204, "no oracle");

    let down = Harness::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/oauth/revoke"))
        .respond_with(iam_error(503, "unavailable"))
        .mount(&down.iam)
        .await;
    let r = down
        .send(json_body(
            post("/api/v1/auth/logout"),
            &json!({"token": REFRESH}),
        ))
        .await;
    assert_eq!(r.status, 503);
    assert_eq!(r.code(), "iam_unavailable");
}

#[tokio::test]
async fn me_reports_the_live_identity() {
    let h = Harness::start().await;
    mount_silicon(&h.iam, ACCESS).await;
    mount_me(&h.iam, "Cleanup").await;
    let r = h
        .send(
            authed("GET", "/api/v1/auth/me", ACCESS)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.header("pragma").as_deref(), Some("no-cache"));
    assert_eq!(
        r.json(),
        json!({
            "authenticated": true, "actor": {"type": "silicon", "public_id": ACTOR}, "display_name": "Cleanup",
            "org_id": ORG, "membership_id": "si:cleanup[tos]", "org_role": "admin", "scopes": FULL_SCOPES,
            "reconsent_required": false, "ting": {"subscribed": false}
        })
    );
    let x = &Harness::requests(&h.iam, "/api/v1/oauth/introspect").await[0];
    assert_eq!(x.headers["x-org-id"], ORG, "introspection is org-bound");
    assert_eq!(
        x.headers["authorization"],
        basic("peek", APP_SECRET).as_str()
    );
}

#[tokio::test]
async fn me_rejects_bad_bearers() {
    let h = Harness::start().await;
    mount_introspect(&h.iam, "oat_revokedToken01", json!({"active": false})).await;
    let mut wrong_client = introspection(ACTOR, ORG, &FULL_SCOPES, None, None);
    wrong_client["client_id"] = json!("dm");
    mount_introspect(&h.iam, "oat_otherClient01", wrong_client).await;
    Mock::given(method("POST"))
        .and(path("/api/v1/oauth/introspect"))
        .and(wiremock::matchers::body_string_contains(
            "token=oat_outage01&",
        ))
        .respond_with(iam_error(503, "unavailable"))
        .mount(&h.iam)
        .await;

    let r = h
        .send(
            authed("GET", "/api/v1/auth/me", "oat_revokedToken01")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, 401);
    assert_eq!(r.code(), "unauthenticated");

    let r = h
        .send(
            authed("GET", "/api/v1/auth/me", "oat_otherClient01")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, 401);
    assert_eq!(r.json()["error"]["details"]["failed_check"], "client_id");

    let r = h
        .send(
            authed("GET", "/api/v1/auth/me", "oat_outage01")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, 503);
    assert_eq!(r.code(), "iam_unavailable");

    let r = h
        .send(Request::get("/api/v1/auth/me").body(Body::empty()).unwrap())
        .await;
    assert_eq!(r.status, 401);

    let r = h
        .send(
            Request::get("/api/v1/auth/me")
                .header("authorization", format!("Bearer {ACCESS}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, 400, "X-Org-ID is required");

    let r = h
        .send(
            Request::get("/api/v1/auth/me")
                .header("authorization", "Bearer cat_directIamToken")
                .header("x-org-id", ORG)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, 401, "only peek access tokens");
}
