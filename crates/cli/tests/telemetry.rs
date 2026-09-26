//! CLI telemetry (BLUEPRINT §6.3–§6.6): one `command.finished` event per
//! invocation to the gateway when nothing opts out; nothing otherwise; never
//! for `peek iam`.

mod common;

use common::Env;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path},
};

async fn gateway() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/web/telemetry"))
        .and(header("x-peek-source", "cli"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    server
}

async fn telemetry_bodies(server: &MockServer) -> Vec<serde_json::Value> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.url.path() == "/api/web/telemetry")
        .filter_map(|r| serde_json::from_slice(&r.body).ok())
        .collect()
}

#[tokio::test]
async fn one_command_finished_event_is_posted() {
    let server = gateway().await;
    let mut env = Env::new(&server.uri());
    env.var("PEEK_TELEMETRY", "1");
    let run = env.run(&["config", "show", "--json"]).await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    let bodies = telemetry_bodies(&server).await;
    assert_eq!(bodies.len(), 1, "{bodies:?}");
    let b = &bodies[0];
    assert_eq!(b["table"], "peekclidaemon");
    let e = &b["events"][0];
    assert_eq!(e["type"], "command.finished");
    assert!(
        e["metadata"]["occurred_at"]
            .as_str()
            .is_some_and(|t| t.ends_with('Z'))
    );
    let d = &e["data"];
    assert_eq!(d["event"], "command.finished");
    assert_eq!(d["source"], "cli");
    assert_eq!(d["outcome"], "ok");
    assert_eq!(d["exit_code"], 0);
    assert_eq!(d["context"]["command"], "config show");
    assert!(d["duration_ms"].as_u64().is_some());
}

#[tokio::test]
async fn errors_carry_their_code() {
    let server = gateway().await;
    let mut env = Env::new(&server.uri());
    env.var("PEEK_TELEMETRY", "on");
    let run = env.run(&["config", "set", r#"{"colour":1}"#]).await;
    assert_eq!(run.code, 2);
    let bodies = telemetry_bodies(&server).await;
    assert_eq!(bodies.len(), 1);
    let d = &bodies[0]["events"][0]["data"];
    assert_eq!(d["outcome"], "error");
    assert_eq!(d["error_code"], "unknown_config_key");
    assert_eq!(d["exit_code"], 2);
    assert!(
        !bodies[0].to_string().contains("colour"),
        "input is never recorded"
    );
}

#[tokio::test]
async fn every_opt_out_is_honoured() {
    let server = gateway().await;
    let mut env = Env::new(&server.uri());
    env.var("PEEK_TELEMETRY", "1");
    // Flag.
    assert_eq!(env.run(&["config", "show", "--no-telemetry"]).await.code, 0);
    // Environment.
    let mut other = Env::new(&server.uri());
    other
        .var("PEEK_TELEMETRY", "1")
        .var("SPACE_STATION_TELEMETRY", "0");
    assert_eq!(other.run(&["config", "show"]).await.code, 0);
    // Home config (the setting run itself is still recorded before the change lands).
    assert_eq!(
        env.run(&["config", "telemetry", "off", "--no-telemetry"])
            .await
            .code,
        0
    );
    assert_eq!(env.run(&["config", "show"]).await.code, 0);
    assert_eq!(env.run(&["docs", "ask"]).await.code, 0);
    // peek iam never posts.
    let mut fresh = Env::new(&server.uri());
    fresh.var("PEEK_TELEMETRY", "1");
    assert_eq!(fresh.run(&["iam", "--json"]).await.code, 0);
    assert!(telemetry_bodies(&server).await.is_empty());
}
