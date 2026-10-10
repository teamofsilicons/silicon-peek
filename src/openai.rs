//! Completed WAV recordings to `OpenAI` transcription; credentials stay server-side.

use crate::{
    error::{ApiError, ApiResult},
    plane::Plane,
    state::AppState,
};
use axum::http::{StatusCode, header};
use reqwest::multipart::{Form, Part};
use serde::Deserialize;
use serde_json::json;
use silicon_peek_client::{ErrorCode, Secret, api::SpeechTranscript};
use std::time::Duration;

pub(crate) const MODEL: &str = "gpt-transcribe";
const RESPONSE_MAX_BYTES: usize = 1024 * 1024;

pub(crate) fn key<'a>(state: &'a AppState, plane: &Plane) -> ApiResult<&'a Secret> {
    let config = &state.0.config.openai;
    let (key, variable) = (&config.api_key, "PEEK_OPENAI_API_KEY");
    key.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::SpeechUnavailable,
            format!("OpenAI transcription is not configured ({variable} is empty)"),
        )
        .with_details(json!({"reason": "not_configured", "provider": "openai"}))
        .with_retryable(false)
    })
}

#[derive(Deserialize)]
struct Transcription {
    text: String,
    #[serde(default)]
    languages: Vec<Language>,
}
#[derive(Deserialize)]
struct Language {
    code: String,
}

fn unavailable(message: &str) -> ApiError {
    ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::SpeechUnavailable,
        message,
    )
    .with_details(json!({"reason": "openai_unavailable", "provider": "openai"}))
    .with_retry_after(5)
}

fn upload(params: &[(String, String)], audio: bytes::Bytes) -> ApiResult<Form> {
    let length = audio.len() as u64;
    let file = Part::stream_with_length(reqwest::Body::from(audio), length)
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .map_err(|_| ApiError::internal("could not prepare the WAV upload"))?;
    let mut form = Form::new()
        .part("file", file)
        .text("model", MODEL)
        .text("response_format", "json");
    let mut languages = Vec::new();
    for (name, value) in params {
        match name.as_str() {
            "keyterm" => form = form.text("keywords[]", value.clone()),
            "language" | "detect_language" if !matches!(value.as_str(), "true" | "false") => {
                let lower = value.to_ascii_lowercase();
                let language = if matches!(lower.as_str(), "zh-cn" | "zh-tw" | "zh-hk") {
                    lower
                } else {
                    lower.split('-').next().unwrap_or_default().to_owned()
                };
                if !languages.contains(&language) {
                    languages.push(language);
                }
            }
            "numerals" if value == "true" => {
                form = form.text("prompt", "Write spoken numbers as digits.");
            }
            _ => {}
        }
    }
    for language in languages {
        form = form.text("languages[]", language);
    }
    Ok(form)
}

pub(crate) async fn listen(
    state: &AppState,
    plane: &Plane,
    params: &[(String, String)],
    audio: bytes::Bytes,
) -> ApiResult<SpeechTranscript> {
    let key = key(state, plane)?;
    let mut response = state
        .0
        .http
        .post(format!(
            "{}/v1/audio/transcriptions",
            state.0.config.openai.base_url
        ))
        .bearer_auth(key.expose())
        .multipart(upload(params, audio)?)
        .timeout(Duration::from_secs(60))
        .send()
        .await
        .map_err(|_| unavailable("OpenAI transcription could not be reached"))?;
    if !response.status().is_success() {
        // Provider errors may echo audio context or credentials; never read them.
        let status = response.status().as_u16();
        let retryable = matches!(status, 408 | 429) || status >= 500;
        let rejected = matches!(status, 400 | 413 | 415 | 422);
        let error = ApiError::new(
            if rejected {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::SERVICE_UNAVAILABLE
            },
            if rejected {
                ErrorCode::InvalidInput
            } else {
                ErrorCode::SpeechUnavailable
            },
            format!("OpenAI transcription answered HTTP {status}"),
        )
        .with_details(json!({"provider":"openai", "openai_status":status,
            "reason":if retryable { "openai_unavailable" } else { "openai_rejected" }}));
        return Err(if retryable {
            let retry = response
                .headers()
                .get(header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(5)
                .clamp(1, 60);
            error.with_retry_after(retry)
        } else {
            error.with_retryable(false)
        });
    }
    if response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(str::trim)
        != Some("application/json")
        || response
            .content_length()
            .is_some_and(|n| n > RESPONSE_MAX_BYTES as u64)
    {
        return Err(unavailable(
            "OpenAI returned an unusable transcription response",
        ));
    }
    let request_id = response
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .filter(|id| id.len() <= 128)
        .map(str::to_owned);
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| unavailable("OpenAI transcription connection failed"))?
    {
        if body.len().saturating_add(chunk.len()) > RESPONSE_MAX_BYTES {
            return Err(unavailable(
                "OpenAI transcription response exceeded the size limit",
            ));
        }
        body.extend_from_slice(&chunk);
    }
    let body: Transcription = serde_json::from_slice(&body)
        .map_err(|_| unavailable("OpenAI returned an invalid transcription response"))?;
    Ok(SpeechTranscript {
        text: body.text,
        request_id,
        detected_language: body
            .languages
            .into_iter()
            .next()
            .map(|language| language.code),
    })
}
