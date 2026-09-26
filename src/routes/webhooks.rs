//! `POST /webhooks/iam`: HMAC-verified IAM notifications (BLUEPRINT §2.10).

use axum::{
    Extension,
    extract::State,
    http::{HeaderMap, StatusCode},
};

use crate::{
    error::ApiResult,
    extract::RawBody,
    state::AppState,
    telemetry::{Event, RequestMeta},
    webhook::{self, Handled},
};

/// Verifies the raw body, dedupes and applies; answers `204` fast.
pub(crate) async fn iam(
    State(state): State<AppState>,
    Extension(meta): Extension<RequestMeta>,
    headers: HeaderMap,
    RawBody(raw): RawBody,
) -> ApiResult<StatusCode> {
    let result = webhook::handle(&state, &headers, &raw).await;
    let outcome = match &result {
        Ok(Handled::Applied { .. }) => "applied",
        Ok(Handled::Duplicate) => "duplicate",
        Ok(Handled::Unroutable) => "unroutable",
        Err(_) => "rejected",
    };
    state.0.telemetry.record(
        &meta,
        Event::new("webhook.received", "webhook.iam")
            .context("status", outcome)
            .outcome(&result),
    );
    result.map(|_| StatusCode::NO_CONTENT)
}
