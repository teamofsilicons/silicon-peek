//! Short-lived Deepgram credentials for direct `ElevenLabs` Voice Agent connections.

use crate::{
    error::{ApiError, ApiResult},
    plane::Plane,
    state::AppState,
};
use axum::http::{StatusCode, header};
use serde::Deserialize;
use serde_json::json;
use silicon_peek_client::{
    ErrorCode, Secret,
    api::{KeySource, SpeechMode, SpeechParams, SpeechProvider, SpeechToken},
};
use std::time::Duration;

const GRANT_MAX_BYTES: usize = 32 * 1024;

fn key<'a>(state: &'a AppState, plane: &Plane) -> ApiResult<&'a Secret> {
    let config = &state.0.config.deepgram;
    let (key, variable) = (&config.api_key, "PEEK_DEEPGRAM_API_KEY");
    key.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::SpeechUnavailable,
            format!("ElevenLabs TTS is not configured ({variable} is empty)"),
        )
        .with_details(json!({"reason":"not_configured", "provider":"elevenlabs"}))
        .with_retryable(false)
    })
}

fn unavailable(message: &str) -> ApiError {
    ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::SpeechUnavailable,
        message,
    )
    .with_details(json!({"reason":"elevenlabs_unavailable", "provider":"elevenlabs"}))
    .with_retry_after(5)
}

#[derive(Deserialize)]
struct Grant {
    access_token: Secret,
    expires_in: u64,
}

fn grant_error(response: &reqwest::Response) -> ApiError {
    // Error bodies can echo credentials; never read or log them.
    let status = response.status().as_u16();
    let retryable = matches!(status, 408 | 429) || status >= 500;
    let error = ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::SpeechUnavailable,
        format!("The TTS token provider answered HTTP {status}"),
    )
    .with_details(json!({"provider":"elevenlabs", "deepgram_status":status,
            "reason":if retryable {"elevenlabs_unavailable"} else {"elevenlabs_rejected"}}));
    if retryable {
        let retry = response
            .headers()
            .get(header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(5)
            .clamp(1, 60);
        error.with_retry_after(retry)
    } else {
        error
            .with_retryable(false)
            .with_hint("configure a Deepgram API key with Member permission or higher")
    }
}

/// Mint a fresh credential; never store it or proxy the client's speech.
pub(crate) async fn token(state: &AppState, plane: &Plane) -> ApiResult<SpeechToken> {
    let key = key(state, plane)?;
    let ttl = state.0.config.deepgram.token_ttl_seconds;
    let mut auth = header::HeaderValue::from_str(&format!("Token {}", key.expose()))
        .map_err(|_| ApiError::internal("invalid TTS credential"))?;
    auth.set_sensitive(true);
    let mut response = state
        .0
        .http
        .post(format!(
            "{}/v1/auth/grant",
            state.0.config.deepgram.base_url
        ))
        .header(header::AUTHORIZATION, auth)
        .json(&json!({"ttl_seconds":ttl}))
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|_| unavailable("The TTS token provider could not be reached"))?;
    if !response.status().is_success() {
        return Err(grant_error(&response));
    }
    if response
        .content_length()
        .is_some_and(|n| n > GRANT_MAX_BYTES as u64)
    {
        return Err(unavailable(
            "The TTS token response exceeded the size limit",
        ));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| unavailable("The TTS token connection failed"))?
    {
        if body.len().saturating_add(chunk.len()) > GRANT_MAX_BYTES {
            return Err(unavailable(
                "The TTS token response exceeded the size limit",
            ));
        }
        body.extend_from_slice(&chunk);
    }
    let grant: Grant = serde_json::from_slice(&body)
        .map_err(|_| unavailable("The TTS token provider returned an invalid response"))?;
    let value = grant.access_token.expose();
    if !(16..=16 * 1024).contains(&value.len())
        || !value.bytes().all(|b| b.is_ascii_graphic())
        || grant.expires_in == 0
        || grant.expires_in > u64::from(ttl)
    {
        return Err(unavailable(
            "The TTS token provider returned an unusable credential",
        ));
    }
    Ok(SpeechToken {
        provider: SpeechProvider::Elevenlabs,
        mode: SpeechMode::Direct,
        access_token: Some(grant.access_token),
        expires_in: grant.expires_in,
        base_url: state.0.config.elevenlabs_agent_url.clone(),
        key_source: KeySource::Peek,
        params: SpeechParams {
            mip_opt_out: true,
            tags: Vec::new(),
        },
    })
}
