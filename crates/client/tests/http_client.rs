//! The stateless peek-server client against a mock server: hop headers,
//! idempotency keys and error decoding (BLUEPRINT §2.9, §5.2, §7.5).

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::time::Duration;

use serde_json::json;
use sha2::{Digest, Sha256};
use silicon_peek_client::{
    ErrorCode, Secret,
    api::{SpeechPurpose, TelemetryBatch, TelemetryEvent, TelemetryTable},
    error::Origin,
    http::Client,
    identity::{ApiUrl, OrgId, TestingSecret},
    ids::{EventId, IdempotencyKey},
};
use wiremock::{
    Mock, MockServer, Request, ResponseTemplate,
    matchers::{body_string, header, header_exists, method, path},
};

fn client(server: &MockServer) -> Client {
    Client::new(&ApiUrl::parse(&server.uri()).unwrap()).unwrap()
}

fn test_secret() -> TestingSecret {
    TestingSecret::parse(&format!("ask_{}", "T".repeat(43))).unwrap()
}

fn session(access: &str) -> serde_json::Value {
    json!({"access_token":access,"refresh_token":"ort_x","token_type":"Bearer","expires_in":1800,
        "scope":"self.identity.read","actor":{"type":"silicon","public_id":"si:cleanup"},
        "org_id":"tos","org_ids":["tos"],"membership_id":"si:cleanup[tos]"})
}

fn has(req: &Request, name: &str) -> bool {
    req.headers.get(name).is_some()
}

#[tokio::test]
async fn login_sends_exactly_the_hop_headers_and_body() {
    let server = MockServer::start().await;
    let key = IdempotencyKey::login("oac_slt");
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login"))
        .and(header("idempotency-key", key.as_str()))
        .and(header("x-org-id", "tos"))
        .and(header("peek-client-version", silicon_peek_client::VERSION))
        .and(header("x-peek-telemetry", "on"))
        .and(header("content-type", "application/json"))
        .and(body_string(r#"{"slt":"oac_slt"}"#))
        .respond_with(ResponseTemplate::new(200).set_body_json(session("oat_1")))
        .expect(1)
        .mount(&server)
        .await;
    let s = client(&server)
        .login(
            &Secret::new("oac_slt"),
            Some(&OrgId::parse("tos").unwrap()),
            &key,
        )
        .await
        .expect("login");
    assert_eq!(s.access_token.expose(), "oat_1");
    let req = &server.received_requests().await.unwrap()[0];
    assert!(!has(req, "authorization"), "login carries no bearer");
    assert!(!has(req, "x-testing-environment-key"));
}

#[tokio::test]
async fn testing_headers_follow_the_hop_table() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/refresh"))
        .respond_with(ResponseTemplate::new(200).set_body_json(session("oat_2")))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/speech/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token":"jwt","expires_in":60,"base_url":"https://api.deepgram.com","key_source":"peek",
            "params":{"mip_opt_out":true,"tags":["peek","testing"]}})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/auth/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "authenticated":true,"actor":{"type":"silicon","public_id":"si:cleanup"},"display_name":null,
            "org_id":"tos","membership_id":"si:cleanup[tos]","org_role":null,"scopes":[],"reconsent_required":false,
            "ting":{"subscribed":false}})))
        .mount(&server)
        .await;
    let c = client(&server)
        .with_testing(test_secret(), Some(7))
        .with_session(Secret::new("oat_1"), OrgId::parse("tos").unwrap())
        .with_telemetry(false);
    c.refresh(&Secret::new("ort_1"), &IdempotencyKey::refresh("ort_1"))
        .await
        .expect("refresh");
    let token = c
        .speech_token(SpeechPurpose::Tts, &IdempotencyKey::generate())
        .await
        .expect("speech token");
    assert_eq!(format!("{:?}", token.access_token), "Some([REDACTED])");
    c.me().await.expect("me");
    let reqs = server.received_requests().await.unwrap();
    let secret = format!("ask_{}", "T".repeat(43));
    for r in &reqs {
        assert_eq!(
            r.headers.get("x-testing-environment-key").unwrap(),
            secret.as_str()
        );
        assert_eq!(r.headers.get("x-peek-telemetry").unwrap(), "off");
    }
    let refresh = &reqs[0];
    assert!(
        !has(refresh, "x-testing-environment-generation"),
        "refresh never carries the generation"
    );
    assert!(
        !has(refresh, "authorization"),
        "refresh carries its token in the body only"
    );
    let speech = &reqs[1];
    assert_eq!(
        speech
            .headers
            .get("x-testing-environment-generation")
            .unwrap(),
        "7"
    );
    assert_eq!(speech.headers.get("authorization").unwrap(), "Bearer oat_1");
    assert_eq!(speech.headers.get("x-org-id").unwrap(), "tos");
    assert!(has(speech, "idempotency-key"));
    let me = &reqs[2];
    assert!(
        !has(me, "x-testing-environment-generation"),
        "reads never carry the generation"
    );
    assert!(!has(me, "idempotency-key"));
}

#[tokio::test]
async fn deliveries_post_the_exact_bytes_with_the_event_key() {
    let server = MockServer::start().await;
    let event = EventId::generate();
    let body = format!(r#"{{"event_id":"{event}","type":"peek.ask.answered"}}"#);
    Mock::given(method("POST"))
        .and(path("/api/v1/deliveries"))
        .and(header("idempotency-key", format!("peek-delivery-{event}").as_str()))
        .and(header("authorization", "Bearer oat_1"))
        .and(body_string(body.clone()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "event_id": event, "ting_id":"msg_1","status":"accepted","silent":true,"replayed":false})))
        .expect(1)
        .mount(&server)
        .await;
    let c = client(&server).with_session(Secret::new("oat_1"), OrgId::parse("tos").unwrap());
    let r = c.deliver(&event, body.as_bytes()).await.expect("delivered");
    assert!(r.silent);
    assert_eq!(r.ting_id, "msg_1");
}

#[tokio::test]
async fn json_error_bodies_decode_into_the_error_model() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/deliveries"))
        .respond_with(
            ResponseTemplate::new(409)
                .insert_header("x-request-id", "req_header")
                .set_body_json(json!({"error":{"code":"recipient_not_registered","message":"si:cleanup is not a Ting recipient",
                    "hint":"peek ting enroll","retryable":false,"details":{"subscribed":false}}})),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/speech/token"))
        .respond_with(
            ResponseTemplate::new(503)
                .insert_header("retry-after", "2")
                .set_body_json(
                    json!({"error":{"code":"speech_unavailable","message":"no key","retryable":true,
                    "request_id":"req_body","details":{"reason":"not_configured"}}}),
                ),
        )
        .mount(&server)
        .await;
    let c = client(&server).with_session(Secret::new("oat_1"), OrgId::parse("tos").unwrap());
    let e = c
        .deliver(&EventId::generate(), b"{}")
        .await
        .expect_err("409");
    assert_eq!(*e.code(), ErrorCode::RecipientNotRegistered);
    assert_eq!(e.exit_code().code(), 4);
    assert_eq!(e.hint(), Some("peek ting enroll"));
    assert_eq!(
        e.request_id(),
        Some("req_header"),
        "falls back to X-Request-ID"
    );
    assert_eq!(e.status(), Some(409));
    assert_eq!(e.origin(), Origin::Server);
    assert_eq!(e.details().unwrap()["subscribed"], false);

    let e = c
        .speech_token(SpeechPurpose::Stt, &IdempotencyKey::generate())
        .await
        .expect_err("503");
    assert_eq!(*e.code(), ErrorCode::SpeechUnavailable);
    assert!(e.retryable());
    assert_eq!(e.request_id(), Some("req_body"));
    assert_eq!(e.retry_after(), Some(Duration::from_secs(2)));
    assert_eq!(e.details().unwrap()["reason"], "not_configured");
    let envelope = e.envelope();
    assert_eq!(envelope["error"]["code"], "speech_unavailable");
}

#[tokio::test]
async fn unknown_codes_and_bodiless_errors_are_classified_by_status() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/auth/me"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_json(json!({"error":{"code":"token_inactive","message":"inactive"}})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/iam"))
        .respond_with(ResponseTemplate::new(502).set_body_string("<html>bad gateway</html>"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/healthz"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
        .mount(&server)
        .await;
    let c = client(&server).with_session(Secret::new("oat_1"), OrgId::parse("tos").unwrap());
    let e = c.me().await.expect_err("401");
    assert_eq!(*e.code(), ErrorCode::Other("token_inactive".into()));
    assert_eq!(e.exit_code().code(), 3);

    let e = c.discover().await.expect_err("502");
    assert_eq!(*e.code(), ErrorCode::BackendUnavailable);
    assert!(e.retryable());
    assert_eq!(e.exit_code().code(), 5);
    assert!(e.message().contains("HTTP 502"));

    let e = c.healthz().await.expect_err("undecodable");
    assert_eq!(*e.code(), ErrorCode::UnexpectedResponse);
    assert_eq!(e.exit_code().code(), 1);
}

#[tokio::test]
async fn transport_failures_are_retryable_backend_unavailable() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let c = Client::new(&ApiUrl::parse(&format!("http://127.0.0.1:{port}")).unwrap()).unwrap();
    let e = c.healthz().await.expect_err("refused");
    assert_eq!(*e.code(), ErrorCode::BackendUnavailable);
    assert!(e.is_transport());
    assert!(e.retryable());
    assert!(e.message().contains(&format!("127.0.0.1:{port}")));

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/healthz"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(2)))
        .mount(&server)
        .await;
    let slow = Client::builder(&ApiUrl::parse(&server.uri()).unwrap())
        .timeout(Duration::from_millis(200))
        .build()
        .unwrap();
    let e = slow.healthz().await.expect_err("timeout");
    assert!(e.is_transport() && e.message().contains("timed out"));
}

#[tokio::test]
async fn bearer_routes_refuse_to_run_without_a_session() {
    let server = MockServer::start().await;
    let e = client(&server).me().await.expect_err("no session");
    assert_eq!(*e.code(), ErrorCode::NotLoggedIn);
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn drawings_are_verified_against_their_etag() {
    let server = MockServer::start().await;
    let script = b"export default function frame(ctx){}".to_vec();
    let sha = hex::encode(Sha256::digest(&script));
    Mock::given(method("PUT"))
        .and(path("/api/v1/drawings/current"))
        .and(header("content-type", "application/javascript"))
        .and(header("x-peek-drawing-sha256", sha.as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"sha256":sha,"bytes":script.len(),"updated_at":"2026-09-26T10:00:00Z"}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/drawings/current"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", format!("\"{sha}\"").as_str())
                .set_body_bytes(script.clone()),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/drawings/current"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "\"deadbeef\"")
                .set_body_bytes(script.clone()),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/drawings/current"))
        .respond_with(
            ResponseTemplate::new(404)
                .set_body_json(json!({"error":{"code":"drawing_not_found","message":"none"}})),
        )
        .mount(&server)
        .await;
    let c = client(&server).with_session(Secret::new("oat_1"), OrgId::parse("tos").unwrap());
    let stored = c.put_drawing(&script).await.expect("put");
    assert_eq!(stored.sha256, sha);
    let d = c.get_drawing().await.expect("get").expect("some");
    assert_eq!(d.bytes, script);
    let e = c.get_drawing().await.expect_err("etag mismatch");
    assert_eq!(*e.code(), ErrorCode::UnexpectedResponse);
    assert!(c.get_drawing().await.expect("404").is_none());
    let e = c
        .put_drawing(&vec![b' '; 256 * 1024 + 1])
        .await
        .expect_err("too large");
    assert_eq!(*e.code(), ErrorCode::DrawingTooLarge);
}

#[tokio::test]
async fn logout_sends_the_bearer_only_when_a_session_is_attached() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/logout"))
        .and(body_string(r#"{"token":"ort_1"}"#))
        .and(header_exists("idempotency-key"))
        .respond_with(ResponseTemplate::new(204))
        .expect(2)
        .mount(&server)
        .await;
    let c = client(&server);
    let key = IdempotencyKey::revoke("ort_1");
    c.logout(&Secret::new("ort_1"), &key)
        .await
        .expect("anonymous");
    c.with_session(Secret::new("oat_1"), OrgId::parse("tos").unwrap())
        .logout(&Secret::new("ort_1"), &key)
        .await
        .expect("with bearer");
    let reqs = server.received_requests().await.unwrap();
    assert!(!has(&reqs[0], "authorization"));
    assert_eq!(
        reqs[1].headers.get("authorization").unwrap(),
        "Bearer oat_1"
    );
    assert_eq!(
        reqs[1].headers.get("idempotency-key").unwrap(),
        key.as_str()
    );
}

#[tokio::test]
async fn telemetry_is_skipped_when_off_and_bounded_when_on() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/web/telemetry"))
        .and(header("x-peek-source", "cli"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let event = TelemetryEvent {
        id: "e1".into(),
        event_type: "command.finished".into(),
        data: json!({"command":"send"}),
        metadata: json!({"occurred_at":"2026-09-26T10:00:00Z"}),
    };
    let batch = TelemetryBatch {
        table: TelemetryTable::Peekclidaemon,
        events: vec![event.clone()],
    };
    let c = client(&server);
    c.with_telemetry(false)
        .telemetry(&batch, "cli", None)
        .await
        .expect("skipped");
    c.telemetry(&batch, "cli", Some(Duration::from_millis(300)))
        .await
        .expect("sent");
    let too_many = TelemetryBatch {
        table: TelemetryTable::Peekclidaemon,
        events: vec![event; 41],
    };
    assert!(c.telemetry(&too_many, "cli", None).await.is_err());
    let body: serde_json::Value = server.received_requests().await.unwrap()[0]
        .body_json()
        .unwrap();
    assert_eq!(body["table"], "peekclidaemon");
}

#[tokio::test]
async fn reports_validate_the_pr_url() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/reports"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id":"rep_1","status":"stored","issue_url":null})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let c = client(&server);
    let mut req = silicon_peek_client::api::ReportRequest {
        message: "send --ask crashed\nsteps…".into(),
        pr: Some("https://github.com/teamofsilicons/silicon-peek/pull/7".into()),
        context: None,
        status: None,
    };
    let r = c
        .report(&req, &IdempotencyKey::generate())
        .await
        .expect("report");
    assert_eq!(r.id, "rep_1");
    req.pr = Some("https://example.com/pr".into());
    assert!(c.report(&req, &IdempotencyKey::generate()).await.is_err());
}

#[tokio::test]
async fn discovery_readiness_enrollment_and_byo_routes() {
    let server = MockServer::start().await;
    let env = uuid::Uuid::now_v7();
    Mock::given(method("GET"))
        .and(path("/api/v1/iam"))
        .and(header_exists("x-testing-environment-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "app_id":"peek","api_version":"v1","api_base_url":server.uri(),"iam_base_url":"https://backend.iam.teamofsilicons.com",
            "testing_environment_id":env,"testing_generation":4,
            "testing_environment":{"id":env,"name":"peek testing","generation":4},
            "compatibility":{"cli":">=0.1.0, <1.0.0","ipc_protocols":[1]}})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/readyz"))
        .respond_with(
            ResponseTemplate::new(503).set_body_json(json!({"status":"not_ready",
            "checks":{"db":"ok","iam_config":"missing","ting_config":"ok","deepgram":"missing"}})),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/ting/recipient"))
        .and(body_string("{}"))
        .and(header("x-testing-environment-generation", "4"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"subscribed":true,"subscription_id":"sub_9"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/orgs/tos/byo/deepgram"))
        .and(body_string(r#"{"api_key":"dg_key"}"#))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"configured":true,"updated_at":"2026-09-26T10:00:00Z","base_url":null}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/orgs/tos/byo/deepgram"))
        .respond_with(
            ResponseTemplate::new(403)
                .set_body_json(json!({"error":{"code":"not_org_admin","message":"admins only"}})),
        )
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/v1/orgs/tos/byo/deepgram"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    let base = client(&server).with_testing(test_secret(), None);
    let d = base.discover().await.expect("discover");
    assert_eq!(d.testing_generation, Some(4));
    assert_eq!(
        d.testing_environment.as_ref().map(|e| e.name.as_str()),
        Some("peek testing")
    );
    let ready = base.readyz().await.expect("503 still decodes");
    assert_eq!(ready.status, "not_ready");
    assert_eq!(ready.checks.iam_config, "missing");

    let org = OrgId::parse("tos").unwrap();
    let c = base
        .with_testing(test_secret(), d.testing_generation)
        .with_session(Secret::new("oat_1"), org.clone());
    let r = c
        .ting_enroll(&IdempotencyKey::generate())
        .await
        .expect("enroll");
    assert_eq!(r.subscription_id, "sub_9");
    let byo = silicon_peek_client::api::ByoDeepgramRequest {
        api_key: Secret::new("dg_key"),
        base_url: None,
    };
    assert!(
        c.set_byo_deepgram(&org, &byo)
            .await
            .expect("put")
            .configured
    );
    let e = c.byo_deepgram(&org).await.expect_err("403");
    assert_eq!(*e.code(), ErrorCode::NotOrgAdmin);
    assert_eq!(e.exit_code().code(), 4);
    c.delete_byo_deepgram(&org).await.expect("delete");
}
