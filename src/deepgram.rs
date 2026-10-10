//! Legacy Deepgram BYO key validation only. Speech never uses these credentials.

use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::http::StatusCode;
use serde_json::json;
use silicon_peek_client::{ErrorCode, Secret};
use std::time::Duration;

/// Keep existing account key management usable without routing speech to Deepgram.
pub(crate) async fn validate_key(state: &AppState, base_url: &str, key: &Secret) -> ApiResult<()> {
    let unavailable = || {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::SpeechUnavailable,
            "Deepgram key validation could not be completed",
        )
        .with_details(json!({"reason":"deepgram_unavailable"}))
        .with_retry_after(5)
    };
    let invalid = || {
        ApiError::invalid_input("Deepgram rejected this API key")
            .with_details(json!({"reason":"key_invalid"}))
    };
    let response = state
        .0
        .http
        .get(format!("{base_url}/v1/auth/token"))
        .header("authorization", format!("Token {}", key.expose()))
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|_| unavailable())?;
    match response.status().as_u16() {
        200 => {}
        401 | 403 => return Err(invalid()),
        _ => return Err(unavailable()),
    }
    // Preserve the administrative key check; never expose or retain the probe JWT.
    let response = state
        .0
        .http
        .post(format!("{base_url}/v1/auth/grant"))
        .header("authorization", format!("Token {}", key.expose()))
        .json(&json!({"ttl_seconds":1}))
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|_| unavailable())?;
    match response.status().as_u16() {
        200 | 403 => Ok(()),
        401 => Err(invalid()),
        402 => Err(
            ApiError::invalid_input("this Deepgram project has no credits left")
                .with_details(json!({"reason":"out_of_credits"})),
        ),
        _ => Err(unavailable()),
    }
}
