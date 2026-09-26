//! `PUT/GET /internal/honeycomb/organizations/{org}/testing-environments/{env}/operations/{op}`:
//! the Honeycomb lifecycle participant (BLUEPRINT §2.9).

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::HeaderMap,
};
use silicon_peek_client::api::participant::{OperationRequest, Receipt};
use uuid::Uuid;

use crate::{
    error::{ApiError, ApiResult},
    extract::RawBody,
    honeycomb,
    state::AppState,
    telemetry::{Event, RequestMeta},
};

/// Applies (or replays) an instruction and returns its receipt.
pub(crate) async fn apply(
    State(state): State<AppState>,
    Extension(meta): Extension<RequestMeta>,
    Path((org, environment, operation)): Path<(String, Uuid, Uuid)>,
    headers: HeaderMap,
    RawBody(raw): RawBody,
) -> ApiResult<Json<Receipt>> {
    honeycomb::authenticate(&state, &headers)?;
    let request: OperationRequest =
        silicon_peek_client::json::from_slice(&raw, "the lifecycle instruction")
            .map_err(ApiError::from_client)?;
    honeycomb::validate(
        &request,
        &org,
        environment,
        operation,
        &state.0.config.iam.app_id,
    )?;
    let action = serde_json::to_value(request.action)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    let result = honeycomb::apply(&state, request).await;
    state.0.telemetry.record(
        &meta,
        Event::new("participant.op", format!("participant.{action}"))
            .context(
                "status",
                result.as_ref().map_or("failed", |r| match r.state {
                    silicon_peek_client::api::participant::ReceiptState::Pending => "pending",
                    silicon_peek_client::api::participant::ReceiptState::Completed => "completed",
                    silicon_peek_client::api::participant::ReceiptState::Failed => "failed",
                }),
            )
            .outcome(&result),
    );
    result.map(Json)
}

/// The stored receipt (lost-response recovery).
pub(crate) async fn receipt(
    State(state): State<AppState>,
    Path((org, environment, operation)): Path<(String, Uuid, Uuid)>,
    headers: HeaderMap,
) -> ApiResult<Json<Receipt>> {
    honeycomb::authenticate(&state, &headers)?;
    honeycomb::stored_receipt(&state, org, environment, operation)
        .await
        .map(Json)
}
