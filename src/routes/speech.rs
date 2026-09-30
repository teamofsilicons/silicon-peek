//! Authenticated speech: short-lived `ElevenLabs` connection tokens and `OpenAI` transcription.
//! No audio, transcript, instructions or provider credentials are logged.

use std::time::Instant;

use axum::{
    Extension, Json,
    extract::{RawQuery, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::json;
use silicon_peek_client::{
    ErrorCode,
    api::{
        KeySource, LISTEN_MAX_BYTES, LISTEN_PARAMS, SpeechMode, SpeechParams, SpeechProvider,
        SpeechPurpose, SpeechToken, SpeechTokenRequest, routes,
    },
};

use crate::{
    auth::{self, Bearer, Principal},
    elevenlabs,
    error::{ApiError, ApiResult},
    extract::{IdemKey, JsonBody, RawBody, single_header},
    openai,
    plane::Plane,
    state::AppState,
    telemetry::{Event, RequestMeta},
};

/// How long a proxy verdict may be reused by peekd (its token cache).
const PROXY_TOKEN_TTL_SECONDS: u64 = 600;
/// At most this many `keyterm` parameters per transcription.
const MAX_KEYTERMS: usize = 100;
/// At most this many `detect_language` parameters per transcription.
const MAX_DETECT_LANGUAGES: usize = 16;

fn rate_limited(what: &str, limit: u32, retry_after: u64) -> ApiError {
    ApiError::new(
        StatusCode::TOO_MANY_REQUESTS,
        ErrorCode::RateLimited,
        format!("too many speech requests {what} (limit {limit} per minute)"),
    )
    .with_hint(
        "reuse a token until it expires (peekd caches them); retry after the indicated delay",
    )
    .with_retry_after(retry_after)
}

/// Authenticates the bearer and takes one unit of the Silicon's and the
/// org's speech budget.
async fn authorize(state: &AppState, plane: &Plane, headers: &HeaderMap) -> ApiResult<Principal> {
    let principal = auth::authenticate(
        plane,
        Bearer::required(headers)?,
        &state.0.config.iam.app_id,
    )
    .await?;
    let ctx = plane.ctx_string();
    let limits = &state.0.limits;
    limits
        .speech_actor
        .take(&format!("{ctx}|{}|{}", principal.org, principal.actor), 1)
        .map_err(|s| rate_limited("for this Silicon", limits.speech_actor.limit(), s))?;
    limits
        .speech_org
        .take(&format!("{ctx}|{}", principal.org), 1)
        .map_err(|s| rate_limited("for this organization", limits.speech_org.limit(), s))?;
    Ok(principal)
}

/// Returns a short-lived TTS connection credential or an STT proxy routing hint.
pub(crate) async fn token(
    State(state): State<AppState>,
    Extension(meta): Extension<RequestMeta>,
    plane: Plane,
    // Required on every POST; routing hints are not replayed.
    IdemKey(_key): IdemKey,
    headers: HeaderMap,
    body: JsonBody<SpeechTokenRequest>,
) -> ApiResult<Json<SpeechToken>> {
    let started = Instant::now();
    let principal = authorize(&state, &plane, &headers).await?;
    let purpose = body.value.purpose;
    let result = async {
        if purpose == SpeechPurpose::Tts {
            return elevenlabs::token(&state, &plane).await;
        }
        openai::key(&state, &plane)?;
        Ok(SpeechToken {
            provider: SpeechProvider::Openai,
            mode: SpeechMode::Proxy,
            access_token: None,
            expires_in: PROXY_TOKEN_TTL_SECONDS,
            base_url: format!("{}{}", state.0.config.public_origin, routes::SPEECH_BASE),
            key_source: KeySource::Peek,
            params: SpeechParams {
                mip_opt_out: true,
                tags: Vec::new(),
            },
        })
    }
    .await;
    let mut event = Event::new(
        "speech.token",
        match purpose {
            SpeechPurpose::Tts => "speech.tts",
            SpeechPurpose::Stt => "speech.stt",
        },
    )
    .actor(&principal.org, &principal.actor)
    .duration(started.elapsed());
    if let Ok(t) = &result {
        event = event.context("key_source", "peek").context(
            "method",
            match t.mode {
                SpeechMode::Direct => "direct",
                SpeechMode::Proxy => "proxy",
            },
        );
    }
    state.0.telemetry.record(&meta, event.outcome(&result));
    result.map(Json)
}

fn parse_bool(name: &str, value: &str) -> ApiResult<()> {
    if matches!(value, "true" | "false") {
        Ok(())
    } else {
        Err(ApiError::invalid_input(format!(
            "`{name}` must be true or false, got `{value}`"
        )))
    }
}

fn parse_language(name: &str, value: &str) -> ApiResult<()> {
    let ok = (1..=35).contains(&value.len())
        && value
            .split('-')
            .all(|p| !p.is_empty() && p.len() <= 8 && p.bytes().all(|b| b.is_ascii_alphanumeric()));
    if ok {
        Ok(())
    } else {
        Err(ApiError::invalid_input(format!(
            "`{name}` must be a BCP 47 language tag such as `en` or `pt-BR`, got `{value}`"
        )))
    }
}

/// Validates the transcription query parameters: only
/// [`LISTEN_PARAMS`]; `detect_language` and `keyterm` may repeat. Returns
/// them in request order.
pub(crate) fn listen_params(raw: Option<&str>) -> ApiResult<Vec<(String, String)>> {
    let mut out: Vec<(String, String)> = Vec::new();
    for (name, value) in url::form_urlencoded::parse(raw.unwrap_or_default().as_bytes()) {
        let (name, value) = (name.into_owned(), value.into_owned());
        if !LISTEN_PARAMS.contains(&name.as_str()) {
            return Err(ApiError::invalid_input(format!(
                "`{name}` is not a speech parameter peek forwards; allowed: {}",
                LISTEN_PARAMS.join(", ")
            ))
            .with_details(json!({"allowed": LISTEN_PARAMS})));
        }
        let repeatable = matches!(name.as_str(), "detect_language" | "keyterm");
        if !repeatable && out.iter().any(|(n, _)| *n == name) {
            return Err(ApiError::invalid_input(format!(
                "`{name}` may appear only once"
            )));
        }
        match name.as_str() {
            "model" => {
                if value != openai::MODEL {
                    return Err(ApiError::invalid_input("model must be gpt-transcribe"));
                }
            }
            "language" => parse_language("language", &value)?,
            "detect_language" if value != "true" && value != "false" => {
                parse_language("detect_language", &value)?;
            }
            "keyterm" => {
                let chars = value.chars().count();
                if chars == 0
                    || chars > 100
                    || value
                        .chars()
                        .any(|c| c.is_control() || matches!(c, '<' | '>'))
                {
                    return Err(ApiError::invalid_input(
                        "each `keyterm` is 1–100 characters without control characters or angle brackets",
                    ));
                }
            }
            "numerals" | "smart_format" => parse_bool(&name, &value)?,
            _ => {}
        }
        out.push((name, value));
    }
    let count = |n: &str| out.iter().filter(|(k, _)| k == n).count();
    if count("keyterm") > MAX_KEYTERMS {
        return Err(ApiError::invalid_input(format!(
            "at most {MAX_KEYTERMS} `keyterm` parameters"
        )));
    }
    if count("detect_language") > MAX_DETECT_LANGUAGES {
        return Err(ApiError::invalid_input(format!(
            "at most {MAX_DETECT_LANGUAGES} `detect_language` parameters"
        )));
    }
    if count("model") == 0 {
        out.insert(0, ("model".to_owned(), openai::MODEL.to_owned()));
    }
    Ok(out)
}

/// `POST /api/v1/speech/listen`: speech to text through peek's key.
pub(crate) async fn listen(
    State(state): State<AppState>,
    Extension(meta): Extension<RequestMeta>,
    plane: Plane,
    IdemKey(_key): IdemKey,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    RawBody(audio): RawBody,
) -> ApiResult<Response> {
    let started = Instant::now();
    let principal = authorize(&state, &plane, &headers).await?;
    let result = async {
        let params = listen_params(query.as_deref())?;
        let content_type = single_header(&headers, header::CONTENT_TYPE.as_str())?
            .unwrap_or("audio/wav")
            .to_owned();
        if !matches!(
            content_type
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "audio/wav" | "audio/x-wav" | "audio/wave"
        ) {
            return Err(ApiError::new(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                ErrorCode::InvalidInput,
                format!("this route takes recorded audio (audio/wav), not `{content_type}`"),
            ));
        }
        if audio.is_empty() {
            return Err(ApiError::invalid_input("the audio body is empty"));
        }
        if audio.len() > LISTEN_MAX_BYTES {
            return Err(ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                ErrorCode::PayloadTooLarge,
                format!(
                    "the audio is {} bytes; the limit is {LISTEN_MAX_BYTES}",
                    audio.len()
                ),
            ));
        }
        openai::listen(&state, &plane, &params, audio).await
    }
    .await;
    let mut event = Event::new("speech.proxy", "speech.stt")
        .actor(&principal.org, &principal.actor)
        .duration(started.elapsed())
        .context(
            "stt_ms",
            u64::try_from(started.elapsed().as_millis()).unwrap_or(0),
        );
    if let Ok(t) = &result {
        event = event.context("key_source", "peek");
        if let Some(id) = &t.request_id {
            event = event.context("stt_request_id", id.clone());
        }
    }
    state.0.telemetry.record(&meta, event.outcome(&result));
    result.map(|t| Json(t).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listen_parameters_are_allow_listed() -> Result<(), Box<dyn std::error::Error>> {
        let p = listen_params(Some(
            "model=gpt-transcribe&smart_format=true&numerals=true&keyterm=Keep+it&keyterm=Delete&detect_language=en&detect_language=hi",
        ))
        .map_err(|e| e.to_string())?;
        assert_eq!(p.len(), 7);
        assert_eq!(p[3], ("keyterm".to_owned(), "Keep it".to_owned()));
        let p = listen_params(Some("language=en")).map_err(|e| e.to_string())?;
        assert_eq!(p[0], ("model".to_owned(), openai::MODEL.to_owned()));
        for bad in [
            "tag=evil",
            "mip_opt_out=false",
            "callback=https://x",
            "model=gpt-transcribe&model=whisper-1",
            "numerals=yes",
            "language=en%20US",
            "keyterm=",
        ] {
            assert!(listen_params(Some(bad)).is_err(), "{bad} must be refused");
        }
        Ok(())
    }
}
