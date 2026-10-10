//! `POST /api/web/telemetry`: the relay for `peekclidaemon`,
//! `peekfrontendanalytics` and `peekfrontendevents` (BLUEPRINT §6.3).
//!
//! Rules: table allowlist; `Origin` must be allowlisted when present (the CLI
//! and peekd send none); 1–40 events and ≤ 64 KiB; `X-Peek-Telemetry: off`,
//! `PEEK_TELEMETRY=off` or a testing key → `204` without forwarding; 6,000
//! events per minute; duplicates count as accepted. No `Idempotency-Key` is
//! required (browsers never send one); Space Station dedupes on `record_id`.

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode, header},
};
use serde_json::Value;
use silicon_peek_client::{
    ErrorCode,
    api::{TelemetryBatch, TelemetryTable, headers},
    telemetry::is_off_value,
};

use crate::{
    error::{ApiError, ApiResult},
    extract::{RawBody, single_header},
    state::AppState,
    telemetry::{RelayOrigin, forward, ingest_records},
};

const MAX_BYTES: usize = 64 * 1024;
const MAX_EVENTS: usize = 40;

fn table_name(table: TelemetryTable) -> &'static str {
    match table {
        TelemetryTable::Peekclidaemon => "peekclidaemon",
        TelemetryTable::Peekfrontendanalytics => "peekfrontendanalytics",
        TelemetryTable::Peekfrontendevents => "peekfrontendevents",
    }
}

/// Strictly parses and validates a batch: allowlisted table, 1–40 events,
/// bounded IDs and types, object metadata.
fn parse_batch(raw: &[u8]) -> ApiResult<TelemetryBatch> {
    let value = silicon_peek_client::json::parse_value(raw).map_err(ApiError::from_client)?;
    let table = value
        .get("table")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !matches!(
        table,
        "peekclidaemon" | "peekfrontendanalytics" | "peekfrontendevents"
    ) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            ErrorCode::TelemetryTableUnavailable,
            format!("the telemetry table `{table}` is not available through this gateway"),
        )
        .with_hint("tables: peekclidaemon, peekfrontendanalytics, peekfrontendevents"));
    }
    let batch: TelemetryBatch = silicon_peek_client::json::from_value(value, "the telemetry batch")
        .map_err(ApiError::from_client)?;
    if batch.events.is_empty() || batch.events.len() > MAX_EVENTS {
        return Err(ApiError::invalid_input(format!(
            "a telemetry batch carries 1–{MAX_EVENTS} events, not {}",
            batch.events.len()
        )));
    }
    for event in &batch.events {
        if event.id.is_empty() || event.id.len() > 100 {
            return Err(ApiError::invalid_input(
                "every event id must be 1–100 bytes",
            ));
        }
        if event.event_type.is_empty() || event.event_type.len() > 160 {
            return Err(ApiError::invalid_input(
                "every event type must be 1–160 bytes",
            ));
        }
        if !event.metadata.is_object() {
            return Err(ApiError::invalid_input(
                "every event's metadata must be an object",
            ));
        }
    }
    Ok(batch)
}

/// Relays one batch.
pub(crate) async fn ingest(
    State(state): State<AppState>,
    headers: HeaderMap,
    RawBody(raw): RawBody,
) -> ApiResult<StatusCode> {
    let config = &state.0.config;
    let opted_out = single_header(&headers, headers::TELEMETRY)?.is_some_and(is_off_value);
    if !config.telemetry.enabled || opted_out {
        return Ok(StatusCode::NO_CONTENT);
    }
    let origin = single_header(&headers, header::ORIGIN.as_str())?;
    if let Some(origin) = origin
        && !config.web_origins.iter().any(|o| o == origin)
    {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            ErrorCode::OriginNotAllowed,
            format!("telemetry from origin `{origin}` is not accepted"),
        ));
    }
    if raw.len() > MAX_BYTES {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            ErrorCode::PayloadTooLarge,
            format!(
                "a telemetry batch is at most {MAX_BYTES} bytes, this one is {}",
                raw.len()
            ),
        ));
    }
    let batch = parse_batch(&raw)?;
    let name = table_name(batch.table);
    let key = match batch.table {
        TelemetryTable::Peekclidaemon => config.telemetry.clidaemon_key.as_ref(),
        TelemetryTable::Peekfrontendanalytics => config.telemetry.analytics_key.as_ref(),
        TelemetryTable::Peekfrontendevents => config.telemetry.events_key.as_ref(),
    };
    let (Some(key), Some(url)) = (key, config.telemetry.url.as_deref()) else {
        tracing::debug!(
            table = name,
            "telemetry table not configured; batch dropped"
        );
        return Ok(StatusCode::NO_CONTENT);
    };
    let count = u32::try_from(batch.events.len()).unwrap_or(u32::MAX);
    let limiter = &state.0.limits.telemetry;
    limiter.take("gateway", count).map_err(|s| {
        ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            ErrorCode::RateLimited,
            format!(
                "the telemetry gateway is over its limit of {} events per minute",
                limiter.limit()
            ),
        )
        .with_retry_after(s)
    })?;
    let source = match single_header(&headers, headers::SOURCE)? {
        Some(s @ ("cli" | "daemon" | "mac")) => s.to_owned(),
        _ if origin.is_some() => "web".to_owned(),
        _ => "unknown".to_owned(),
    };
    let relay = RelayOrigin {
        user_agent: single_header(&headers, header::USER_AGENT.as_str())?
            .map(|ua| ua.chars().take(160).collect()),
        source,
    };
    let records = ingest_records(key, name, batch.events, &relay);
    forward(&state.0.http, url, records).await?;
    Ok(StatusCode::NO_CONTENT)
}
