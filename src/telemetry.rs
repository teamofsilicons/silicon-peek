//! Space Station telemetry (BLUEPRINT §6).
//!
//! - `peekbackend` is recorded directly through an [`EventSink`] (a
//!   `space_station::SpaceClient` in production, a fake in tests). A missing
//!   or malformed key disables it with one warning; it never fails startup.
//! - The gateway `POST /api/web/telemetry` relays the three client tables to
//!   Space Station's HTTP ingest; table keys exist only on the backend (D17).
//! - Nothing is recorded for a request that sent `X-Peek-Telemetry: off`, for
//!   testing contexts, or when `PEEK_TELEMETRY=off`.
//!
//! Records carry the §6.4 envelope. Actor IDs are hashed (D28); texts,
//! tokens and secrets are never recorded; `context` is allowlisted.

use std::{sync::Arc, time::Duration};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use silicon_peek_client::{
    ErrorCode, Secret,
    api::TelemetryEvent,
    identity::{AccountId, ActorId, ActorType},
    telemetry::{actor_hash, scrub_context},
    timestamp::Timestamp,
};
use uuid::Uuid;

use crate::{
    config::TelemetryConfig,
    error::{ApiError, ApiResult},
};

/// Where backend events go. `SpaceClient` in production; tests inject a fake.
pub trait EventSink: Send + Sync {
    /// Records one event; never blocks and never fails the caller.
    fn record(&self, event: Value);
    /// Waits briefly for queued events to be handed off; `true` on success.
    fn flush(&self) -> bool {
        true
    }
}

impl EventSink for space_station::SpaceClient {
    fn record(&self, event: Value) {
        space_station::SpaceClient::record(self, event);
    }

    fn flush(&self) -> bool {
        space_station::SpaceClient::flush(self)
    }
}

/// Builds the `peekbackend` recorder from configuration: `None` (with one
/// warning) when telemetry is off, the key is missing or the URL is invalid.
#[must_use]
pub fn backend_sink(config: &TelemetryConfig) -> Option<Arc<dyn EventSink>> {
    if !config.enabled {
        return None;
    }
    let key = config.backend_key.as_ref()?;
    let url = config.url.as_deref()?;
    if let Err(e) = std::fs::create_dir_all(&config.home) {
        tracing::warn!(error = %e, home = %config.home.display(), "cannot create PEEK_TELEMETRY_HOME; backend telemetry is disabled");
        return None;
    }
    let _ = rustls::crypto::ring::default_provider().install_default();
    match space_station::SpaceClient::builder(key.expose())
        .url(url)
        .home(&config.home)
        .flush_timeout(Duration::from_millis(100))
        .on_error(|_| {})
        .build()
    {
        Ok(client) => Some(Arc::new(client)),
        Err(e) => {
            tracing::warn!(error = %e, "PEEK_BACKEND_TABLE_KEY was refused by the Space Station client; backend telemetry is disabled");
            None
        }
    }
}

/// Per-request facts every handler may need (set by the request middleware).
#[derive(Clone, Debug)]
pub(crate) struct RequestMeta {
    /// The server-generated request ID.
    pub(crate) request_id: String,
    /// `false` when the caller sent `X-Peek-Telemetry: off`.
    pub(crate) telemetry: bool,
    /// Whether the request selected a testing environment.
    /// `X-Peek-Trace-Id`, when well-formed.
    pub(crate) trace_id: Option<String>,
}

/// One backend event.
pub(crate) struct Event {
    name: &'static str,
    step: String,
    outcome: &'static str,
    duration_ms: Option<u64>,
    error_code: Option<String>,
    actor: Option<(ActorType, String)>,
    context: Map<String, Value>,
}

impl Event {
    /// An event with `outcome: ok`.
    pub(crate) fn new(name: &'static str, step: impl Into<String>) -> Self {
        Self {
            name,
            step: step.into(),
            outcome: "ok",
            duration_ms: None,
            error_code: None,
            actor: None,
            context: Map::new(),
        }
    }

    /// Marks the event failed with a stable error code.
    pub(crate) fn failed(mut self, code: &ErrorCode) -> Self {
        self.outcome = "error";
        self.error_code = Some(code.as_str().to_owned());
        self
    }

    /// Sets the outcome from a result.
    pub(crate) fn outcome<T>(self, result: &ApiResult<T>) -> Self {
        match result {
            Ok(_) => self,
            Err(e) => self.failed(e.code()),
        }
    }

    /// Adds the duration.
    pub(crate) fn duration(mut self, duration: Duration) -> Self {
        self.duration_ms = Some(u64::try_from(duration.as_millis()).unwrap_or(u64::MAX));
        self
    }

    /// Adds the (hashed) actor.
    pub(crate) fn actor(mut self, account: &AccountId, actor: &ActorId) -> Self {
        self.actor = Some((actor.actor_type(), actor_hash(account, actor)));
        self
    }

    /// Adds an allowlisted context value (others are dropped on record).
    pub(crate) fn context(mut self, key: &'static str, value: impl Into<Value>) -> Self {
        self.context.insert(key.to_owned(), value.into());
        self
    }
}

/// The backend recorder.
#[derive(Clone)]
pub(crate) struct Telemetry {
    sink: Option<Arc<dyn EventSink>>,
    environment: &'static str,
    instance_id: String,
}

impl Telemetry {
    /// A recorder writing to `sink`.
    pub(crate) fn new(sink: Option<Arc<dyn EventSink>>, environment: &'static str) -> Self {
        Self {
            sink,
            environment,
            instance_id: Uuid::now_v7().to_string(),
        }
    }

    /// Records `event` unless the request opted out or is a testing context.
    pub(crate) fn record(&self, meta: &RequestMeta, event: Event) {
        if !meta.telemetry {
            return;
        }
        let Some(sink) = &self.sink else {
            return;
        };
        let mut context = event.context;
        scrub_context(&mut context);
        let progress = u8::from(!event.name.ends_with("started"));
        sink.record(json!({
            "schema_version": 1,
            "app": "peek",
            "service": "peek-backend",
            "source": "backend",
            "version": silicon_peek_client::VERSION,
            "environment": self.environment,
            "instance_id": self.instance_id,
            "trace_id": meta.trace_id,
            "request_id": meta.request_id,
            "step": event.step,
            "event": event.name,
            "progress": progress,
            "outcome": event.outcome,
            "duration_ms": event.duration_ms,
            "error_code": event.error_code,
            "actor": event.actor.map(|(kind, hash)| json!({"kind": kind.as_str(), "hash": hash})),
            "context": context,
        }));
    }
}

/// `record_id = Uuid(sha256("{key}:{event_id}")[..16])`: stable per event,
/// namespaced by the secret key so clients cannot collide with each other.
pub(crate) fn record_id(key: &Secret, event_id: &str) -> Uuid {
    let digest = Sha256::digest(format!("{}:{event_id}", key.expose()).as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    Uuid::from_bytes(bytes)
}

/// Facts about the relaying request, stamped into `metadata.server`.
pub(crate) struct RelayOrigin {
    pub(crate) user_agent: Option<String>,
    pub(crate) source: String,
}

/// Builds the Space Station ingest records for a validated batch; events a
/// client tagged `environment: testing` are dropped (they never reach the
/// production tables).
pub(crate) fn ingest_records(
    key: &Secret,
    table: &str,
    events: Vec<TelemetryEvent>,
    origin: &RelayOrigin,
) -> Vec<Value> {
    let received_at = Timestamp::now().to_rfc3339();
    events
        .into_iter()
        .filter(|e| e.data.get("environment").and_then(Value::as_str) != Some("testing"))
        .map(|event| {
            let event_ts_ms = event
                .metadata
                .get("occurred_at")
                .and_then(Value::as_str)
                .and_then(|s| Timestamp::parse(s).ok())
                .map_or_else(|| Timestamp::now().unix_ms(), Timestamp::unix_ms);
            let mut data = event.data;
            if let Some(context) = data.get_mut("context").and_then(Value::as_object_mut) {
                scrub_context(context);
            }
            let mut metadata = match event.metadata {
                Value::Object(map) => map,
                _ => Map::new(),
            };
            metadata.insert(
                "server".to_owned(),
                json!({
                    "client_reported": true,
                    "received_at": received_at,
                    "user_agent": origin.user_agent,
                    "source": origin.source,
                }),
            );
            json!({
                "key": key.expose(),
                "metadata": {
                    "record_id": record_id(key, &event.id),
                    "table_id": table,
                    "event_ts_ms": event_ts_ms,
                },
                "record": {
                    "type": event.event_type,
                    "data": data,
                    "metadata": metadata,
                },
            })
        })
        .collect()
}

/// Posts records to `POST {url}/api/ingest`. Duplicates count as accepted.
pub(crate) async fn forward(
    http: &reqwest::Client,
    url: &str,
    records: Vec<Value>,
) -> ApiResult<()> {
    if records.is_empty() {
        return Ok(());
    }
    let body = json!({"batch_id": Uuid::new_v4().to_string(), "records": records});
    let unavailable = |why: String| {
        ApiError::new(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::BackendUnavailable,
            format!("Space Station did not accept the telemetry batch: {why}"),
        )
        .with_hint("telemetry is best effort; drop the batch or retry later")
        .with_retry_after(30)
    };
    let response = http
        .post(format!("{url}/api/ingest"))
        .timeout(Duration::from_secs(5))
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            unavailable(if e.is_timeout() {
                "timed out".to_owned()
            } else {
                "could not connect".to_owned()
            })
        })?;
    let status = response.status();
    let bytes = response.bytes().await.unwrap_or_default();
    if !status.is_success() {
        return Err(unavailable(format!("HTTP {status}")));
    }
    let ack: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    match ack.get("status").and_then(Value::as_str) {
        Some("ok") => Ok(()),
        Some("rejected") => {
            let rejected = ack
                .get("rejected")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let real: Vec<&Value> = rejected
                .iter()
                .filter(|r| r.get("code").and_then(Value::as_str) != Some("duplicate"))
                .collect();
            if real.is_empty() && ack.get("code").is_none() {
                Ok(())
            } else {
                let codes: Vec<&str> = real
                    .iter()
                    .filter_map(|r| r.get("code").and_then(Value::as_str))
                    .chain(ack.get("code").and_then(Value::as_str))
                    .collect();
                tracing::warn!(?codes, "Space Station rejected relayed telemetry");
                Err(ApiError::new(
                    axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                    ErrorCode::TelemetryRejected,
                    format!(
                        "Space Station rejected {} telemetry record(s)",
                        codes.len().max(1)
                    ),
                )
                .with_details(json!({"codes": codes})))
            }
        }
        _ => Err(unavailable("unexpected acknowledgement".to_owned())),
    }
}
