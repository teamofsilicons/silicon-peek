//! Speech for peekd (BLUEPRINT §5.2 "Deepgram minting", amended by the
//! speech proxy mode):
//!
//! - `POST /api/v1/speech/token`: a short-lived Deepgram JWT
//!   (`{"mode":"direct",…}`), or — when the resolved key may not mint JWTs
//!   (`/v1/auth/grant` answers 401/403; the verdict is cached per key for ten
//!   minutes) — `{"mode":"proxy","base_url":"<public origin>/api/v1/speech",…}`.
//! - `POST /api/v1/speech/speak`: `{"text","model","sample_rate"?}` →
//!   Deepgram Aura-2 linear16 audio, streamed back unbuffered with
//!   `dg-request-id` / `dg-char-count`.
//! - `POST /api/v1/speech/listen`: raw audio (≤ 4 MiB) plus allow-listed
//!   query parameters → Deepgram Nova-3's JSON.
//!
//! Every route is Bearer (`oat_`) authenticated, rate limited per Silicon and
//! per org (one budget shared by mints and proxied calls), and never logs
//! audio, text or tokens. Every Deepgram request carries `mip_opt_out=true`
//! and `tag=peek` plus the environment tag.

use std::time::Instant;

use axum::{
    Extension, Json,
    body::Body,
    extract::{RawQuery, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::json;
use silicon_peek_client::{
    ErrorCode, Secret,
    api::{
        KeySource, LISTEN_MAX_BYTES, LISTEN_PARAMS, SPEAK_SAMPLE_RATES, SpeechMode, SpeechParams,
        SpeechPurpose, SpeechSpeakRequest, SpeechToken, SpeechTokenRequest,
        headers as peek_headers, routes,
    },
    identity::OrgId,
    schema::send::{check_speak, check_voice},
};

use crate::{
    auth::{self, Bearer, Principal},
    crypto::byo_aad,
    deepgram,
    error::{ApiError, ApiResult},
    extract::{IdemKey, JsonBody, RawBody, single_header},
    plane::Plane,
    state::AppState,
    store,
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

/// The key a call uses: the org's BYO key if configured, else peek's key for
/// this plane (production and testing keys never mix). No fallback.
pub(crate) async fn resolve_key(
    state: &AppState,
    plane: &Plane,
    org: &OrgId,
) -> ApiResult<(Secret, String, KeySource)> {
    let (ctx, org_s) = (plane.ctx_string(), org.to_string());
    let byo = {
        let (ctx, org_s) = (ctx.clone(), org_s.clone());
        plane
            .db
            .call(move |conn| Ok(store::byo::get(conn, &ctx, &org_s)?))
            .await?
    };
    let config = &state.0.config.deepgram;
    if let Some(row) = byo {
        let key = state
            .0
            .sealer
            .open_string(&byo_aad(&ctx, &org_s), &row.sealed)?;
        let base = match row.base_url {
            None => config.base_url.clone(),
            // Re-checked on every use: a base URL stored before the operator
            // narrowed PEEK_BYO_DEEPGRAM_HOSTS (or before the policy existed)
            // is refused, never silently replaced by peek's key.
            Some(stored) => config.byo_hosts.check_origin(&stored).map_err(|why| {
                ApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    ErrorCode::SpeechUnavailable,
                    format!("the org's own Deepgram key is configured with a base URL this peek-server no longer allows: {why}"),
                )
                .with_hint(deepgram::org_key_hint(&org_s))
                .with_details(json!({"reason": "org_base_url_not_allowed", "key_source": KeySource::Org}))
                .with_retryable(false)
            })?,
        };
        return Ok((Secret::new(key), base, KeySource::Org));
    }
    let key = if plane.is_testing() {
        config.test_api_key.clone()
    } else {
        config.api_key.clone()
    };
    let key = key.ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::SpeechUnavailable,
            if plane.is_testing() {
                "speech is not configured for testing environments (PEEK_DEEPGRAM_TEST_API_KEY is empty)"
            } else {
                "speech is not configured on this peek-server (PEEK_DEEPGRAM_API_KEY is empty)"
            },
        )
        .with_hint(format!("speech degrades to a text pill; an org admin can configure the org's own key with peek --org {org_s} org byo deepgram set --key-file -"))
        .with_details(json!({"reason": "not_configured"}))
        .with_retryable(false)
    })?;
    Ok((key, config.base_url.clone(), KeySource::Peek))
}

/// `tag=` values: `peek` and the environment (`testing` for a testing
/// plane, else `PEEK_ENVIRONMENT`: `production` or `development`).
fn tags(state: &AppState, plane: &Plane) -> Vec<String> {
    vec![
        "peek".to_owned(),
        if plane.is_testing() {
            "testing"
        } else {
            state.0.config.environment.as_str()
        }
        .to_owned(),
    ]
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

fn key_source_str(source: KeySource) -> &'static str {
    match source {
        KeySource::Peek => "peek",
        KeySource::Org => "org",
    }
}

/// Mints a JWT (≤ `PEEK_DEEPGRAM_TOKEN_TTL_SECONDS`), or answers the proxy
/// verdict. The JWT is never logged.
pub(crate) async fn token(
    State(state): State<AppState>,
    Extension(meta): Extension<RequestMeta>,
    plane: Plane,
    // Required on every POST; minting is not replayed (a JWT is never stored).
    IdemKey(_key): IdemKey,
    headers: HeaderMap,
    body: JsonBody<SpeechTokenRequest>,
) -> ApiResult<Json<SpeechToken>> {
    let started = Instant::now();
    let principal = authorize(&state, &plane, &headers).await?;
    let purpose = body.value.purpose;
    let result = async {
        let (key, base_url, source) = resolve_key(&state, &plane, &principal.org).await?;
        let params = SpeechParams {
            mip_opt_out: true,
            tags: tags(&state, &plane),
        };
        let fingerprint = deepgram::key_fingerprint(&base_url, &key);
        let proxy = |expires_in: u64| SpeechToken {
            mode: SpeechMode::Proxy,
            access_token: None,
            expires_in,
            base_url: format!("{}{}", state.0.config.public_origin, routes::SPEECH_BASE),
            key_source: source,
            params: params.clone(),
        };
        if let Some(left) = state.grant_forbidden_for(&fingerprint) {
            return Ok(proxy(left.as_secs().clamp(1, PROXY_TOKEN_TTL_SECONDS)));
        }
        let ttl = state.0.config.deepgram.token_ttl_seconds;
        match deepgram::grant(&state, &base_url, &key, ttl).await {
            Ok(grant) => Ok(SpeechToken {
                mode: SpeechMode::Direct,
                access_token: Some(grant.access_token),
                expires_in: grant.expires_in,
                base_url,
                key_source: source,
                params,
            }),
            Err(deepgram::Failure::Status {
                status: 401 | 403, ..
            }) => {
                tracing::info!(
                    key_source = key_source_str(source),
                    "the Deepgram key may not mint JWTs; speech goes through the proxy"
                );
                state.remember_grant_forbidden(fingerprint);
                Ok(proxy(PROXY_TOKEN_TTL_SECONDS))
            }
            Err(failure) => Err(deepgram::mint_error(
                source,
                principal.org.as_str(),
                &failure,
            )),
        }
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
        event = event
            .context("key_source", key_source_str(t.key_source))
            .context(
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

fn copy_header(from: &reqwest::header::HeaderMap, to: &mut HeaderMap, name: &'static str) {
    if let Some(v) = from
        .get(name)
        .and_then(|v| v.to_str().ok())
        .filter(|v| v.len() <= 256)
        .and_then(|v| HeaderValue::from_str(v).ok())
    {
        to.insert(name, v);
    }
}

/// `POST /api/v1/speech/speak`: text to speech through peek's key, streamed.
pub(crate) async fn speak(
    State(state): State<AppState>,
    Extension(meta): Extension<RequestMeta>,
    plane: Plane,
    IdemKey(_key): IdemKey,
    headers: HeaderMap,
    body: JsonBody<SpeechSpeakRequest>,
) -> ApiResult<Response> {
    let started = Instant::now();
    let principal = authorize(&state, &plane, &headers).await?;
    let request = body.value;
    let result = async {
        check_speak(&request.text).map_err(ApiError::from_client)?;
        check_voice(&request.model).map_err(ApiError::from_client)?;
        let sample_rate = request.sample_rate.unwrap_or(24_000);
        if !SPEAK_SAMPLE_RATES.contains(&sample_rate) {
            return Err(ApiError::invalid_input(format!(
                "sample_rate {sample_rate} is not supported; use one of {SPEAK_SAMPLE_RATES:?}"
            )));
        }
        let (key, base_url, source) = resolve_key(&state, &plane, &principal.org).await?;
        let upstream = deepgram::speak(
            &state,
            &base_url,
            &key,
            &request.model,
            &request.text,
            sample_rate,
            &tags(&state, &plane),
        )
        .await
        .map_err(|f| deepgram::proxy_error(source, principal.org.as_str(), "/v1/speak", &f))?;
        Ok((upstream, source))
    }
    .await;
    let mut event = Event::new("speech.proxy", "speech.tts")
        .actor(&principal.org, &principal.actor)
        .duration(started.elapsed())
        .context("tts_model", request.model.clone())
        .context("speak_chars", request.text.chars().count());
    if let Ok((upstream, source)) = &result {
        event = event
            .context("key_source", key_source_str(*source))
            .context(
                "tts_ttfb_ms",
                u64::try_from(started.elapsed().as_millis()).unwrap_or(0),
            );
        if let Some(id) = upstream
            .headers()
            .get(peek_headers::DG_REQUEST_ID)
            .and_then(|v| v.to_str().ok())
        {
            event = event.context("dg_request_id", id.to_owned());
        }
    }
    state.0.telemetry.record(&meta, event.outcome(&result));
    let (upstream, _) = result?;
    let mut response_headers = HeaderMap::new();
    let content_type = upstream
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| HeaderValue::from_str(v).ok())
        .unwrap_or_else(|| HeaderValue::from_static("audio/l16"));
    response_headers.insert(header::CONTENT_TYPE, content_type);
    copy_header(
        upstream.headers(),
        &mut response_headers,
        peek_headers::DG_REQUEST_ID,
    );
    copy_header(
        upstream.headers(),
        &mut response_headers,
        peek_headers::DG_CHAR_COUNT,
    );
    // Stream Deepgram's body through as it arrives: no buffering, no copy.
    let body = Body::from_stream(upstream.bytes_stream());
    Ok((StatusCode::OK, response_headers, body).into_response())
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

/// Validates the allow-listed Deepgram parameters (§ speech proxy): only
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
                let ok = (1..=64).contains(&value.len())
                    && value
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
                if !ok {
                    return Err(ApiError::invalid_input(format!(
                        "`model` must look like `nova-3`, got `{value}`"
                    )));
                }
            }
            "language" => parse_language("language", &value)?,
            "detect_language" if value != "true" && value != "false" => {
                parse_language("detect_language", &value)?;
            }
            "keyterm" => {
                let chars = value.chars().count();
                if chars == 0 || chars > 100 || value.chars().any(char::is_control) {
                    return Err(ApiError::invalid_input(
                        "each `keyterm` is 1–100 characters without control characters",
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
        out.insert(0, ("model".to_owned(), "nova-3".to_owned()));
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
        if !content_type.to_ascii_lowercase().starts_with("audio/") {
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
        let (key, base_url, source) = resolve_key(&state, &plane, &principal.org).await?;
        let t = deepgram::listen(
            &state,
            &base_url,
            &key,
            params,
            &content_type,
            audio,
            &tags(&state, &plane),
        )
        .await
        .map_err(|f| deepgram::proxy_error(source, principal.org.as_str(), "/v1/listen", &f))?;
        Ok((t, source))
    }
    .await;
    let mut event = Event::new("speech.proxy", "speech.stt")
        .actor(&principal.org, &principal.actor)
        .duration(started.elapsed())
        .context(
            "stt_ms",
            u64::try_from(started.elapsed().as_millis()).unwrap_or(0),
        );
    if let Ok((t, source)) = &result {
        event = event.context("key_source", key_source_str(*source));
        if let Some(id) = &t.request_id {
            event = event.context("dg_request_id", id.clone());
        }
    }
    state.0.telemetry.record(&meta, event.outcome(&result));
    let (t, _) = result?;
    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    if let Some(v) = t
        .request_id
        .as_deref()
        .and_then(|v| HeaderValue::from_str(v).ok())
    {
        response_headers.insert(peek_headers::DG_REQUEST_ID, v);
    }
    Ok((StatusCode::OK, response_headers, Body::from(t.body)).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listen_parameters_are_allow_listed() -> Result<(), Box<dyn std::error::Error>> {
        let p = listen_params(Some(
            "model=nova-3&smart_format=true&numerals=true&keyterm=Keep+it&keyterm=Delete&detect_language=en&detect_language=hi",
        ))
        .map_err(|e| e.to_string())?;
        assert_eq!(p.len(), 7);
        assert_eq!(p[3], ("keyterm".to_owned(), "Keep it".to_owned()));
        let p = listen_params(Some("language=en")).map_err(|e| e.to_string())?;
        assert_eq!(p[0], ("model".to_owned(), "nova-3".to_owned()));
        for bad in [
            "tag=evil",
            "mip_opt_out=false",
            "callback=https://x",
            "model=nova-3&model=nova-2",
            "numerals=yes",
            "language=en%20US",
            "keyterm=",
        ] {
            assert!(listen_params(Some(bad)).is_err(), "{bad} must be refused");
        }
        Ok(())
    }
}
