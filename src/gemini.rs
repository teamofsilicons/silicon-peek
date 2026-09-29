//! Gemini TTS: validate delivery metadata and relay SSE without decoding audio.

use std::{fmt::Write as _, time::Duration};

use axum::{
    body::Body,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use silicon_peek_client::{ErrorCode, Secret, api::SpeechSpeakRequest};

use crate::{
    error::{ApiError, ApiResult},
    plane::Plane,
    state::AppState,
};

/// The API model is independent of the selected voice.
pub(crate) const MODEL: &str = "gemini-3.8-flash-tts";

pub(crate) fn key<'a>(state: &'a AppState, plane: &Plane) -> ApiResult<&'a Secret> {
    let config = &state.0.config.gemini;
    let (key, variable) = if plane.is_testing() {
        (&config.test_api_key, "PEEK_GEMINI_TEST_API_KEY")
    } else {
        (&config.api_key, "PEEK_GEMINI_API_KEY")
    };
    key.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::SpeechUnavailable,
            format!("Gemini TTS is not configured ({variable} is empty)"),
        )
        .with_details(json!({"reason": "not_configured", "provider": "gemini"}))
        .with_retryable(false)
    })
}

fn accent_error() -> ApiError {
    ApiError::invalid_input(
        "accent tags must be paired, non-empty and unnested, for example <indian accent>Anuv Jain</indian accent>",
    )
}

/// Peek's paired accent shorthand becomes Google's scoped style metadata.
/// Other tags, including Google's singleton vocal tags, stay verbatim.
fn content(text: &str, instructions: &str, language: Option<&str>) -> ApiResult<Vec<Value>> {
    let mut blocks = Vec::new();
    let mut active = None;
    let mut start = 0;
    let mut cursor = 0;
    let mut push = |text: &str, accent: Option<&str>| {
        if text.is_empty() {
            return;
        }
        let mut style = instructions.to_owned();
        if let Some(language) = language {
            let _ = write!(style, "\nSpeak in language {language}.");
        }
        if let Some(accent) = accent {
            let _ = write!(style, "\nUse an {accent} for this text.");
        }
        let mut block = json!({"type": "text", "text": text});
        if !style.trim().is_empty() {
            block["annotations"] = json!([{"type": "speech_metadata", "style": style.trim()}]);
        }
        blocks.push(block);
    };
    while let Some(offset) = text[cursor..].find('<') {
        let open = cursor + offset;
        let Some(length) = text[open..].find('>') else {
            if text[open..].contains("accent") {
                return Err(accent_error());
            }
            break;
        };
        let end = open + length + 1;
        let tag = &text[open + 1..end - 1];
        let name = tag.strip_prefix('/').unwrap_or(tag);
        if name.ends_with(" accent") {
            if !(8..=64).contains(&name.len())
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphabetic() || b == b' ' || b == b'-')
            {
                return Err(accent_error());
            }
            if tag.starts_with('/') {
                if active != Some(name) || text[start..open].trim().is_empty() {
                    return Err(accent_error());
                }
                push(&text[start..open], active);
                active = None;
            } else {
                if active.is_some() {
                    return Err(accent_error());
                }
                push(&text[start..open], None);
                active = Some(name);
            }
            start = end;
        }
        cursor = end;
    }
    if active.is_some() {
        return Err(accent_error());
    }
    push(&text[start..], None);
    Ok(blocks)
}

pub(crate) async fn speak(
    state: &AppState,
    plane: &Plane,
    request: &SpeechSpeakRequest,
) -> ApiResult<Response> {
    let key = key(state, plane)?;
    let blocks = content(
        &request.text,
        request.voice_instructions.as_deref().unwrap_or_default(),
        request.language.as_deref(),
    )?;
    let upstream = state
        .0
        .http
        .post(format!(
            "{}/v1beta/interactions",
            state.0.config.gemini.base_url
        ))
        .header("x-goog-api-key", key.expose())
        .header(header::ACCEPT, "text/event-stream")
        .timeout(Duration::from_secs(180))
        .json(&json!({
            "model": MODEL,
            "input": [{"type": "user_input", "content": blocks}],
            "response_format": {"type": "audio", "mime_type": "audio/l16", "sample_rate": 24000},
            "generation_config": {"speech_config": [{"voice": request.model}]},
            "stream": true,
            "store": false
        }))
        .send()
        .await
        .map_err(|_| unavailable("Gemini TTS could not be reached"))?;
    let status = upstream.status();
    if !status.is_success() {
        // Never read error bodies: they may echo the transcript, instructions or key.
        let details = json!({"provider": "gemini", "gemini_status": status.as_u16(),
            "reason": if status.is_client_error() && status.as_u16() != 429 { "gemini_rejected" } else { "gemini_unavailable" }});
        if matches!(status.as_u16(), 400 | 413 | 422) {
            return Err(ApiError::invalid_input(
                "Gemini refused the speech request; check the voice and instructions",
            )
            .with_details(details));
        }
        let error = ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::SpeechUnavailable,
            format!("Gemini TTS answered HTTP {}", status.as_u16()),
        )
        .with_details(details);
        return Err(if matches!(status.as_u16(), 401 | 403 | 404) {
            error.with_retryable(false)
        } else {
            let retry = upstream
                .headers()
                .get(header::RETRY_AFTER)
                .and_then(|h| h.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(5);
            error.with_retry_after(retry.clamp(1, 60))
        });
    }
    if !upstream
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|v| {
            v.split(';')
                .next()
                .is_some_and(|v| v.trim() == "text/event-stream")
        })
    {
        return Err(unavailable(
            "Gemini TTS returned an unexpected response format",
        ));
    }
    // Dropping the response on client cancellation also drops the upstream stream.
    Ok((
        [
            (header::CONTENT_TYPE, "text/event-stream"),
            (header::CACHE_CONTROL, "no-store"),
            (header::HeaderName::from_static("x-accel-buffering"), "no"),
        ],
        Body::from_stream(upstream.bytes_stream()),
    )
        .into_response())
}

fn unavailable(message: &str) -> ApiError {
    ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::SpeechUnavailable,
        message,
    )
    .with_details(json!({"reason": "gemini_unavailable", "provider": "gemini"}))
    .with_retry_after(5)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accent_scopes_preserve_vocal_tags_and_reject_broken_markup() -> ApiResult<()> {
        let blocks = content(
            "Play <indian accent>Anuv Jain</indian accent>. <sigh>",
            "Warm",
            Some("en"),
        )?;
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[1]["text"], "Anuv Jain");
        assert_eq!(
            blocks[1]["annotations"][0]["style"],
            "Warm\nSpeak in language en.\nUse an indian accent for this text."
        );
        assert_eq!(blocks[2]["text"], ". <sigh>");
        assert!(
            !blocks[2]["annotations"][0]["style"]
                .as_str()
                .unwrap_or_default()
                .contains("accent")
        );
        for text in [
            "<indian accent>hi",
            "</indian accent>",
            "<indian accent></indian accent>",
            "<indian accent><british accent>hi</british accent></indian accent>",
            "<indian accent>hi</british accent>",
            "<indian accent",
        ] {
            assert!(content(text, "", None).is_err(), "{text}");
        }
        assert_eq!(
            content("Hi <laughing> there", "", None)?,
            vec![json!({"type": "text", "text": "Hi <laughing> there"})]
        );
        Ok(())
    }
}
