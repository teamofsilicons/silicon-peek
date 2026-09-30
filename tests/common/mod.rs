//! Shared fixtures: a peek-server router wired to local wiremock fakes of
//! IAM, Ting, Deepgram, GitHub, Space Station and Honeycomb, with temp-dir
//! databases. Nothing here talks to a real service.

#![allow(
    dead_code,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::missing_panics_doc
)]

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Request, StatusCode},
};
use base64::Engine as _;
use serde_json::{Value, json};
use silicon_peek::{
    app,
    config::Config,
    state::{AppState, Limits},
    telemetry::EventSink,
};
use tower::ServiceExt as _;
use uuid::Uuid;
use wiremock::{
    Match, Mock, MockServer, ResponseTemplate,
    matchers::{body_string_contains, header, method, path},
};

pub const APP_SECRET: &str = "ask_prodSecretprodSecretprodSecretprodSecretpro";
pub const TEST_SECRET: &str = "ask_testSecrettestSecrettestSecrettestSecrettes";
pub const TING_TEST_SECRET: &str = "ask_tingSecrettingSecrettingSecrettingSecrettin";
pub const WEBHOOK_SECRET: &str = "whs_0123456789abcdef0123456789abcdef0123456789abcdef";
pub const SERVICE_TOKEN: &str = "hc-service-token-0123456789abcdef-0123";
pub const ROOT_KEY: &str = "RootKey0123456789RootKey01234567";
pub const PEEK_DG_KEY: &str = "dg-peek-key-0123456789";
pub const PEEK_DG_TEST_KEY: &str = "dg-test-key-0123456789";
pub const GITHUB_TOKEN: &str = "github_pat_0123456789abcdefghij";
pub const IDEM: &str = "peek-test-key-0000000001";
pub const ACCESS: &str = "oat_cleanupAccessToken01";
pub const REFRESH: &str = "ort_cleanupRefreshToken01";
pub const ACTOR: &str = "si:cleanup";
pub const ORG: &str = "tos";
pub const FULL_SCOPES: [&str; 6] = [
    "obo:ting:subscriptions.register",
    "obo:ting:subscriptions.revoke",
    "obo:ting:tings.send",
    "self.identity.read",
    "self.membership.read",
    "self.profile.read",
];

pub fn env_id() -> Uuid {
    Uuid::parse_str("0192f2d2-7c9e-7cc0-8b2e-6f3a2b1c0d9e").unwrap()
}

/// Captures backend telemetry.
#[derive(Default)]
pub struct Sink(pub Mutex<Vec<Value>>);

impl EventSink for Sink {
    fn record(&self, event: Value) {
        self.0.lock().unwrap().push(event);
    }
}

pub struct Harness {
    pub dir: tempfile::TempDir,
    pub iam: MockServer,
    pub ting: MockServer,
    pub deepgram: MockServer,
    pub openai: MockServer,
    pub github: MockServer,
    pub spacestation: MockServer,
    pub honeycomb: MockServer,
    pub state: AppState,
    pub app: Router,
    pub sink: Arc<Sink>,
}

pub struct Response {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl Response {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|_| panic!("not JSON: {}", String::from_utf8_lossy(&self.body)))
    }

    pub fn code(&self) -> String {
        self.json()["error"]["code"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    }

    pub fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    }
}

/// Tweaks applied to the default environment before the config is read.
pub type Env = HashMap<&'static str, String>;

impl Harness {
    pub async fn start() -> Self {
        Self::with(|_| {}, Limits::default()).await
    }

    pub async fn with(tweak: impl FnOnce(&mut Env), limits: Limits) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let iam = MockServer::start().await;
        let ting = MockServer::start().await;
        let deepgram = MockServer::start().await;
        let openai = MockServer::start().await;
        let github = MockServer::start().await;
        let spacestation = MockServer::start().await;
        let honeycomb = MockServer::start().await;
        let mut env: Env = HashMap::from([
            (
                "PEEK_DATABASE_PATH",
                dir.path().join("peek.sqlite").display().to_string(),
            ),
            (
                "PEEK_TEST_DATABASE_PATH",
                dir.path().join("testing.sqlite").display().to_string(),
            ),
            (
                "PEEK_PUBLIC_ORIGIN",
                "https://backend.peek.teamofsilicons.com".to_owned(),
            ),
            (
                "PEEK_WEB_ORIGINS",
                "https://peek.teamofsilicons.com".to_owned(),
            ),
            ("PEEK_ENVIRONMENT", "development".to_owned()),
            ("PEEK_IAM_BASE_URL", iam.uri()),
            ("PEEK_IAM_APP_SECRET", APP_SECRET.to_owned()),
            ("PEEK_IAM_WEBHOOK_SECRET", WEBHOOK_SECRET.to_owned()),
            ("PEEK_IAM_WEBHOOK_KEY_VERSION", "1".to_owned()),
            ("PEEK_TING_BASE_URL", ting.uri()),
            ("PEEK_TING_REQUEST_TIMEOUT_SECONDS", "5".to_owned()),
            ("PEEK_HONEYCOMB_URL", honeycomb.uri()),
            ("PEEK_HONEYCOMB_SERVICE_TOKEN", SERVICE_TOKEN.to_owned()),
            ("PEEK_ENCRYPTION_KEY", "ab".repeat(32)),
            ("PEEK_DEEPGRAM_API_KEY", PEEK_DG_KEY.to_owned()),
            ("PEEK_DEEPGRAM_TEST_API_KEY", PEEK_DG_TEST_KEY.to_owned()),
            ("PEEK_DEEPGRAM_BASE_URL", deepgram.uri()),
            (
                "PEEK_ELEVENLABS_AGENT_URL",
                "wss://agent.deepgram.com/v1/agent/converse".to_owned(),
            ),
            ("PEEK_OPENAI_API_KEY", "openai-production-key".to_owned()),
            ("PEEK_OPENAI_TEST_API_KEY", "openai-testing-key".to_owned()),
            ("PEEK_OPENAI_BASE_URL", openai.uri()),
            ("PEEK_GITHUB_ISSUES_TOKEN", GITHUB_TOKEN.to_owned()),
            ("PEEK_GITHUB_API_URL", github.uri()),
            ("PEEK_TELEMETRY", "on".to_owned()),
            // Never let the IAM SDK look for a real ~/.silicon-iam telemetry key.
            ("PEEK_IAM_SDK_TELEMETRY", "off".to_owned()),
            (
                "PEEK_CLIDAEMON_TABLE_KEY",
                format!("table-peekclidaemon-{}", "c".repeat(32)),
            ),
            (
                "PEEK_FRONTEND_ANALYTICS_TABLE_KEY",
                format!("table-peekfrontendanalytics-{}", "a".repeat(32)),
            ),
            ("PEEK_TELEMETRY_URL", spacestation.uri()),
            (
                "PEEK_TELEMETRY_HOME",
                dir.path().join("telemetry").display().to_string(),
            ),
        ]);
        tweak(&mut env);
        let config = Config::from_lookup(&|k| env.get(k).cloned()).expect("valid test config");
        let sink = Arc::new(Sink::default());
        let state = AppState::with_limits(config, Some(sink.clone() as Arc<dyn EventSink>), limits)
            .expect("state");
        let app = app::router(state.clone());
        Self {
            dir,
            iam,
            ting,
            deepgram,
            openai,
            github,
            spacestation,
            honeycomb,
            state,
            app,
            sink,
        }
    }

    pub async fn send(&self, request: Request<Body>) -> Response {
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = axum::body::to_bytes(response.into_body(), 32 << 20)
            .await
            .unwrap()
            .to_vec();
        Response {
            status,
            headers,
            body,
        }
    }

    pub fn events(&self) -> Vec<Value> {
        self.sink.0.lock().unwrap().clone()
    }

    /// Requests received by a mock whose path is `p`.
    pub async fn requests(server: &MockServer, p: &str) -> Vec<wiremock::Request> {
        server
            .received_requests()
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.url.path() == p)
            .collect()
    }
}

/// A JSON POST with an Idempotency-Key.
pub fn post(path: &str) -> axum::http::request::Builder {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .header("idempotency-key", IDEM)
}

/// Finishes a builder with a JSON body.
pub fn json_body(builder: axum::http::request::Builder, body: &Value) -> Request<Body> {
    builder.body(Body::from(body.to_string())).unwrap()
}

/// A bearer request builder.
pub fn authed(method_: &str, path: &str, token: &str) -> axum::http::request::Builder {
    Request::builder()
        .method(method_)
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .header("x-org-id", ORG)
}

// ---------------------------------------------------------------- IAM fakes

/// Matches requests that do NOT carry a header.
pub struct NoHeader(pub &'static str);

impl Match for NoHeader {
    fn matches(&self, request: &wiremock::Request) -> bool {
        !request.headers.contains_key(self.0)
    }
}

pub fn basic(user: &str, secret: &str) -> String {
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{user}:{secret}"))
    )
}

/// An authorization snapshot.
pub fn authorization(
    actor: &str,
    org: &str,
    scopes: &[&str],
    role: Option<&str>,
    env: Option<Uuid>,
) -> Value {
    let kind = if actor.starts_with("si:") {
        "silicon"
    } else {
        "carbon"
    };
    json!({
        "actor_type": kind, "public_id": actor, "organization_id": Uuid::now_v7(), "org_id": org,
        "membership_id": format!("{actor}[{org}]"), "membership_version": 1, "authorization_epoch": 1,
        "audience": "peek", "testing_environment_id": env, "scopes": scopes, "org_role": role, "tags": null
    })
}

/// An active org-bound introspection.
pub fn introspection(
    actor: &str,
    org: &str,
    scopes: &[&str],
    role: Option<&str>,
    env: Option<Uuid>,
) -> Value {
    let kind = if actor.starts_with("si:") {
        "silicon"
    } else {
        "carbon"
    };
    json!({
        "active": true, "public_id": actor, "actor_type": kind, "client_id": "peek", "org_id": org,
        "membership_id": format!("{actor}[{org}]"), "scope": scopes.join(" "), "audience": "peek",
        "issued_at": 1_790_000_000_i64, "expires_at": 4_000_000_000_i64, "authorization_epoch": 1,
        "authorization": authorization(actor, org, scopes, role, env)
    })
}

/// Mounts production `POST /api/v1/oauth/introspect` for `token` (org-bound).
pub async fn mount_introspect(iam: &MockServer, token: &str, response: Value) {
    Mock::given(method("POST"))
        .and(path("/api/v1/oauth/introspect"))
        .and(body_string_contains(format!("token={token}&")))
        .and(wiremock::matchers::header_exists("x-org-id"))
        .and(NoHeader("x-testing-application"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response))
        .mount(iam)
        .await;
}

/// Mounts the testing-plane introspection (selected by peek's test secret).
pub async fn mount_introspect_testing(iam: &MockServer, token: &str, response: Value) {
    Mock::given(method("POST"))
        .and(path("/api/v1/oauth/introspect"))
        .and(body_string_contains(format!("token={token}&")))
        .and(wiremock::matchers::header_exists("x-org-id"))
        .and(header(
            "x-testing-application",
            basic("peek", TEST_SECRET).as_str(),
        ))
        .and(header("authorization", basic("peek", TEST_SECRET).as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(response))
        .mount(iam)
        .await;
}

/// The standard active Silicon with every scope, admin in `tos`.
pub async fn mount_silicon(iam: &MockServer, token: &str) {
    mount_introspect(
        iam,
        token,
        introspection(ACTOR, ORG, &FULL_SCOPES, Some("admin"), None),
    )
    .await;
}

/// Mounts the unscoped introspection `oauth().authorizations()` performs.
pub async fn mount_authorizations(iam: &MockServer, token: &str, authorizations: Value) {
    Mock::given(method("POST"))
        .and(path("/api/v1/oauth/introspect"))
        .and(body_string_contains(format!("token={token}&")))
        .and(NoHeader("x-org-id"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "active": true, "client_id": "peek", "audience": "peek", "authorizations": authorizations
        })))
        .mount(iam)
        .await;
}

/// An `OAuthTokenResponse`.
pub fn token_response(access: &str, refresh: &str, scopes: &[&str]) -> Value {
    json!({
        "access_token": access, "refresh_token": refresh, "token_type": "Bearer", "expires_in": 1800,
        "scope": scopes.join(" "), "actor": {"type": "silicon", "public_id": ACTOR}
    })
}

/// Mounts `POST /api/v1/app-auth/tokens` for a form body containing `needle`.
pub async fn mount_token_exchange(iam: &MockServer, needle: &str, template: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path("/api/v1/app-auth/tokens"))
        .and(body_string_contains(needle))
        .respond_with(template)
        .mount(iam)
        .await;
}

pub fn iam_error(status: u16, code: &str) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_json(json!({
        "error": {"code": code, "message": format!("fixture {code}"), "request_id": Uuid::now_v7()}
    }))
}

/// Mounts `GET /api/v1/me` returning a display name.
pub async fn mount_me(iam: &MockServer, display_name: &str) {
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "principal_id": ACTOR, "type": "silicon", "display_name": display_name, "version": 1
        })))
        .mount(iam)
        .await;
}

/// Ting's OBO catalog.
pub async fn mount_catalog(iam: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/api/v1/obo-access/applications/ting/endpoints"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "application": {"app_id": "ting", "org_id": "tos"},
            "endpoints": [
                {"endpoint_id": "subscriptions.register", "path": "/v1/subscriptions", "critical": true, "metadata": {}, "ttl_seconds": 60},
                {"endpoint_id": "subscriptions.revoke", "path": "/v1/subscriptions/revoke", "critical": true, "metadata": {}, "ttl_seconds": 60},
                {"endpoint_id": "tings.send", "path": "/v1/tings", "critical": true, "metadata": {}, "ttl_seconds": 60}
            ]
        })))
        .mount(iam)
        .await;
}

pub fn proof_body(proof: &str, testing: Option<Value>) -> Value {
    let expires = time::OffsetDateTime::now_utc() + time::Duration::seconds(60);
    let mut v = json!({
        "access_proof": proof, "proof_id": Uuid::now_v7(), "expires_in": 60,
        "expires_at": expires.format(&time::format_description::well_known::Rfc3339).unwrap()
    });
    if let Some(t) = testing {
        v["testing_context"] = t;
    }
    v
}

/// Mounts OBO exchanges signed with the production app secret: each call
/// gets a distinct proof `proof-<n>`.
pub async fn mount_exchange(iam: &MockServer, testing: Option<Value>) {
    mount_exchange_for(iam, APP_SECRET, testing).await;
}

/// As [`mount_exchange`], for exchanges authenticated with `secret`.
pub async fn mount_exchange_for(iam: &MockServer, secret: &str, testing: Option<Value>) {
    let counter = Arc::new(Mutex::new(0_u32));
    Mock::given(method("POST"))
        .and(path("/api/v1/obo-access/exchanges"))
        .and(header("authorization", basic("peek", secret).as_str()))
        .respond_with(move |_: &wiremock::Request| {
            let mut n = counter.lock().unwrap();
            *n += 1;
            ResponseTemplate::new(200)
                .set_body_json(proof_body(&format!("proof-{n}"), testing.clone()))
        })
        .mount(iam)
        .await;
}

/// Ting accepts registrations.
pub async fn mount_ting_register(ting: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/v1/subscriptions"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "sub_1", "app_id": "peek", "for": ACTOR, "active": true, "required_delivery": false
        })))
        .mount(ting)
        .await;
}

/// Ting accepts sends, echoing the key (status 202, or 200 for a replay).
pub async fn mount_ting_send(ting: &MockServer, status: u16, silent: bool) {
    Mock::given(method("POST"))
        .and(path("/v1/tings"))
        .respond_with(move |req: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
            ResponseTemplate::new(status).set_body_json(json!({
                "id": "msg_1", "created_at": "2026-09-26T10:00:00Z", "status": "accepted",
                "key": body["key"], "silent": silent
            }))
        })
        .mount(ting)
        .await;
}

pub fn ting_error(status: u16, code: &str) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_json(json!({
        "error": {"code": code, "message": format!("fixture {code}"), "hint": null, "retryable": status >= 500}
    }))
}

// ------------------------------------------------------------- testing plane

/// IAM's testing context for peek's test secret, and Ting's for its secret.
pub async fn mount_testing_contexts(iam: &MockServer) {
    let environment = json!({
        "environment_id": env_id(), "org_id": ORG, "name": "peek testing", "version": 1, "key_generation": 1,
        "created_at": "2026-09-26T00:00:00Z", "creator_type": "carbon", "creator_id": "c:peek-admin"
    });
    Mock::given(method("GET"))
        .and(path("/api/v1/application/testing-context"))
        .and(header("x-testing-application", basic("peek", TEST_SECRET).as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "environment_id": env_id(),
            "application": {"app_id": "peek", "base_url": "https://backend.peek.teamofsilicons.com",
                "app_scope": {"iam": [], "external": []}, "webhook_scope": ["membership"], "testing_idle_days": 30},
            "environment": environment
        })))
        .mount(iam)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/application/testing-context"))
        .and(header("authorization", basic("ting", TING_TEST_SECRET).as_str()))
        .and(header("x-testing-environment-key", ROOT_KEY))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "environment_id": env_id(),
            "application": {"app_id": "ting", "base_url": "https://backend.ting.teamofsilicons.com",
                "app_scope": {"iam": [], "external": []}, "webhook_scope": [], "testing_idle_days": 15}
        })))
        .mount(iam)
        .await;
}

/// A participant instruction.
pub fn operation(
    action: &str,
    revision: u64,
    generation: u64,
    key_version: u64,
    key: &str,
) -> Value {
    json!({
        "operation_id": Uuid::now_v7(), "environment_id": env_id(), "org_id": ORG, "app_id": "peek",
        "environment_revision": revision, "generation": generation, "key_version": key_version,
        "action": action, "testing_key": key, "snapshot": {}, "reason": "fixture"
    })
}

pub fn participant_request(op: &Value) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri(format!(
            "/internal/honeycomb/organizations/{}/testing-environments/{}/operations/{}",
            op["org_id"].as_str().unwrap(),
            op["environment_id"].as_str().unwrap(),
            op["operation_id"].as_str().unwrap()
        ))
        .header("authorization", format!("Bearer {SERVICE_TOKEN}"))
        .header("content-type", "application/json")
        .body(Body::from(op.to_string()))
        .unwrap()
}

impl Harness {
    /// Prepares peek in the fixture testing environment (generation 1).
    pub async fn prepare_environment(&self) {
        let r = self
            .send(participant_request(&operation(
                "prepare", 1, 1, 1, ROOT_KEY,
            )))
            .await;
        assert_eq!(
            r.status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&r.body)
        );
        assert_eq!(r.json()["state"], "completed");
    }
}

/// Adds the testing headers to a builder.
pub fn testing(
    builder: axum::http::request::Builder,
    generation: Option<u64>,
) -> axum::http::request::Builder {
    let builder = builder.header("x-testing-environment-key", TEST_SECRET);
    match generation {
        Some(g) => builder.header("x-testing-environment-generation", g.to_string()),
        None => builder,
    }
}
