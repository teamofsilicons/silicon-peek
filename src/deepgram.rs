//! Deepgram access (BLUEPRINT §5.2 "Deepgram minting", notes/speech §3).
//!
//! peek-server holds the Deepgram keys. Normally it only calls
//! `POST {base}/v1/auth/grant` and peekd calls Deepgram directly with the
//! returned JWT. When a key may not mint JWTs (`/v1/auth/grant` answers 401
//! or 403, e.g. a key without the Member role), the token route answers
//! `{"mode":"proxy"}` and peekd sends its TTS/STT requests through
//! `POST /api/v1/speech/speak|listen`, which this module forwards to
//! `/v1/speak` and `/v1/listen` with `Authorization: Token <key>`. Audio and
//! text are streamed through and never logged or stored; JWTs are never
//! logged or stored. Every Deepgram request carries `mip_opt_out=true` and
//! `tag=peek` plus the environment tag. There is no silent fallback between
//! an org's BYO key and peek's.

use std::time::Duration;

use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use silicon_peek_client::{ErrorCode, Secret, api::KeySource};

use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};

const TIMEOUT: Duration = Duration::from_secs(10);

/// A minted token.
pub(crate) struct Grant {
    pub(crate) access_token: Secret,
    pub(crate) expires_in: u64,
}

#[derive(Deserialize)]
struct GrantBody {
    access_token: String,
    #[serde(default)]
    expires_in: Option<u64>,
}

/// How long a TTS request through the proxy may take, streaming included
/// (2000 characters synthesize in well under this).
const SPEAK_TIMEOUT: Duration = Duration::from_secs(180);
/// How long a transcription through the proxy may take.
const LISTEN_TIMEOUT: Duration = Duration::from_secs(60);

/// The most of an error body peek-server reads: Deepgram's envelope is a few
/// hundred bytes, and anything bigger is not worth holding in memory.
const ERROR_BODY_MAX: usize = 8 * 1024;
/// The most of a `/v1/auth/grant` body peek-server reads (a JWT and a TTL).
const GRANT_BODY_MAX: usize = 32 * 1024;
/// The most of a transcription peek-server reads (Deepgram's JSON for 4 MiB
/// of audio is far smaller).
const LISTEN_BODY_MAX: usize = 16 * 1024 * 1024;

/// How a Deepgram call failed.
pub(crate) enum Failure {
    /// Deepgram answered with this status.
    Status {
        status: u16,
        retry_after: Option<u64>,
        /// Deepgram's `err_code: err_msg` (never request content).
        detail: Option<String>,
        /// `dg-request-id`.
        request_id: Option<String>,
    },
    /// Deepgram could not be reached.
    Transport(&'static str),
    /// Deepgram answered 200 with an unusable body.
    Malformed,
}

fn transport(e: &reqwest::Error) -> Failure {
    Failure::Transport(if e.is_timeout() {
        "timed out"
    } else if e.is_connect() {
        "could not connect"
    } else {
        "the connection failed"
    })
}

fn retry_after(response: &reqwest::Response) -> Option<u64> {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
}

fn dg_request_id(response: &reqwest::Response) -> Option<String> {
    response
        .headers()
        .get("dg-request-id")
        .and_then(|v| v.to_str().ok())
        .filter(|v| v.len() <= 128)
        .map(str::to_owned)
}

/// Deepgram's error envelope, reduced to `code: message` (both shapes).
fn describe(body: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    let code = ["err_code", "category", "error_code"]
        .iter()
        .find_map(|k| v.get(*k).and_then(|x| x.as_str()));
    let msg = ["err_msg", "message", "details"]
        .iter()
        .find_map(|k| v.get(*k).and_then(|x| x.as_str()));
    let text = match (code, msg) {
        (Some(c), Some(m)) => format!("{c}: {m}"),
        (Some(c), None) => c.to_owned(),
        (None, Some(m)) => m.to_owned(),
        (None, None) => return None,
    };
    Some(text.chars().take(300).collect())
}

/// Why a capped read stopped.
enum CappedRead {
    /// The body is (or announced itself as) bigger than the cap.
    TooLarge,
    /// The connection failed while reading.
    Transport(Failure),
}

/// Reads at most `max` bytes of `response`'s body, chunk by chunk: a body
/// that announces or reaches more than `max` stops the read at once (the
/// connection is dropped), so an upstream can never make peek-server buffer
/// more than `max`. Deepgram's base URL can be an org's choice, so no upstream
/// body is trusted to be small.
async fn read_capped(mut response: reqwest::Response, max: usize) -> Result<Vec<u8>, CappedRead> {
    let limit = u64::try_from(max).unwrap_or(u64::MAX);
    if response.content_length().is_some_and(|n| n > limit) {
        return Err(CappedRead::TooLarge);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| CappedRead::Transport(transport(&e)))?
    {
        if body.len().saturating_add(chunk.len()) > max {
            return Err(CappedRead::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Reads a failed response into [`Failure::Status`]. At most
/// [`ERROR_BODY_MAX`] bytes of the body are read; a bigger or broken body
/// just loses its detail.
async fn status_failure(response: reqwest::Response) -> Failure {
    let status = response.status().as_u16();
    let retry_after = retry_after(&response);
    let request_id = dg_request_id(&response);
    let body = read_capped(response, ERROR_BODY_MAX)
        .await
        .unwrap_or_default();
    Failure::Status {
        status,
        retry_after,
        detail: describe(&body),
        request_id,
    }
}

/// A stable, non-reversible fingerprint of a key at a base URL (the grant
/// verdict cache is keyed by it; the key itself is never kept twice).
pub(crate) fn key_fingerprint(base_url: &str, key: &Secret) -> String {
    let mut h = Sha256::new();
    h.update(base_url.as_bytes());
    h.update([0]);
    h.update(key.expose().as_bytes());
    hex::encode(h.finalize())
}

/// The query every proxied Deepgram request ends with: `tag=peek`, the
/// environment tag, and `mip_opt_out=true` (always).
pub(crate) fn common_query(tags: &[String]) -> Vec<(String, String)> {
    let mut q: Vec<(String, String)> = Vec::new();
    for t in tags {
        if !q.iter().any(|(_, v)| v == t) {
            q.push(("tag".to_owned(), t.clone()));
        }
    }
    q.push(("mip_opt_out".to_owned(), "true".to_owned()));
    q
}

/// `POST {base}/v1/speak` with peek's (or the org's) key. Returns the
/// response once Deepgram answered 200 with raw audio; the caller streams
/// its body through unbuffered.
pub(crate) async fn speak(
    state: &AppState,
    base_url: &str,
    key: &Secret,
    model: &str,
    text: &str,
    sample_rate: u32,
    tags: &[String],
) -> Result<reqwest::Response, Failure> {
    let mut query = vec![
        ("model".to_owned(), model.to_owned()),
        ("encoding".to_owned(), "linear16".to_owned()),
        ("container".to_owned(), "none".to_owned()),
        ("sample_rate".to_owned(), sample_rate.to_string()),
    ];
    query.extend(common_query(tags));
    let response = state
        .0
        .http
        .post(format!("{base_url}/v1/speak"))
        .query(&query)
        .timeout(SPEAK_TIMEOUT)
        .header(
            reqwest::header::AUTHORIZATION,
            format!("Token {}", key.expose()),
        )
        .json(&json!({ "text": text }))
        .send()
        .await
        .map_err(|e| transport(&e))?;
    if response.status().as_u16() != 200 {
        return Err(status_failure(response).await);
    }
    Ok(response)
}

/// A finished transcription: Deepgram's JSON bytes and request ID.
pub(crate) struct Transcription {
    pub(crate) body: bytes::Bytes,
    pub(crate) request_id: Option<String>,
}

/// `POST {base}/v1/listen` with the allow-listed `params` (already
/// validated) plus [`common_query`].
pub(crate) async fn listen(
    state: &AppState,
    base_url: &str,
    key: &Secret,
    params: Vec<(String, String)>,
    content_type: &str,
    audio: bytes::Bytes,
    tags: &[String],
) -> Result<Transcription, Failure> {
    let mut query = params;
    query.extend(common_query(tags));
    let response = state
        .0
        .http
        .post(format!("{base_url}/v1/listen"))
        .query(&query)
        .timeout(LISTEN_TIMEOUT)
        .header(
            reqwest::header::AUTHORIZATION,
            format!("Token {}", key.expose()),
        )
        .header(reqwest::header::CONTENT_TYPE, content_type)
        .body(audio)
        .send()
        .await
        .map_err(|e| transport(&e))?;
    if response.status().as_u16() != 200 {
        return Err(status_failure(response).await);
    }
    let request_id = dg_request_id(&response);
    let body = match read_capped(response, LISTEN_BODY_MAX).await {
        Ok(body) => bytes::Bytes::from(body),
        Err(CappedRead::TooLarge) => return Err(Failure::Malformed),
        Err(CappedRead::Transport(failure)) => return Err(failure),
    };
    Ok(Transcription { body, request_id })
}

/// `POST {base}/v1/auth/grant` with `Authorization: Token <key>`.
pub(crate) async fn grant(
    state: &AppState,
    base_url: &str,
    key: &Secret,
    ttl_seconds: u32,
) -> Result<Grant, Failure> {
    let response = state
        .0
        .http
        .post(format!("{base_url}/v1/auth/grant"))
        .timeout(TIMEOUT)
        .header(
            reqwest::header::AUTHORIZATION,
            format!("Token {}", key.expose()),
        )
        .json(&json!({"ttl_seconds": ttl_seconds}))
        .send()
        .await
        .map_err(|e| transport(&e))?;
    if response.status().as_u16() != 200 {
        return Err(status_failure(response).await);
    }
    let body = match read_capped(response, GRANT_BODY_MAX).await {
        Ok(body) => body,
        Err(CappedRead::TooLarge) => return Err(Failure::Malformed),
        Err(CappedRead::Transport(failure)) => return Err(failure),
    };
    let body: GrantBody = serde_json::from_slice(&body).map_err(|_| Failure::Malformed)?;
    if body.access_token.is_empty() || body.access_token.len() > 16 * 1024 {
        return Err(Failure::Malformed);
    }
    Ok(Grant {
        access_token: Secret::new(body.access_token),
        expires_in: body
            .expires_in
            .filter(|e| (1..=3600).contains(e))
            .unwrap_or(u64::from(ttl_seconds)),
    })
}

/// What [`validate_key`] says for any transport failure.
const NO_ANSWER: &str = "no answer (the connection failed or timed out)";

/// Checks a key before it is stored as an org's BYO key: `GET /v1/auth/token`
/// (is it a key?) and `POST /v1/auth/grant {"ttl_seconds":1}` (may it mint,
/// i.e. Member role or higher?).
pub(crate) async fn validate_key(state: &AppState, base_url: &str, key: &Secret) -> ApiResult<()> {
    let unreachable = |why: &str| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::SpeechUnavailable,
            format!("peek-server could not reach Deepgram at {base_url} to check the key: {why}"),
        )
        .with_details(json!({"reason": "deepgram_unavailable"}))
        .with_retry_after(5)
    };
    let response = state
        .0
        .http
        .get(format!("{base_url}/v1/auth/token"))
        .timeout(TIMEOUT)
        .header(
            reqwest::header::AUTHORIZATION,
            format!("Token {}", key.expose()),
        )
        .send()
        .await
        // One message for every transport failure: which of "refused",
        // "timed out" or "reset" happened is not the org admin's business
        // and would make this check a port probe.
        .map_err(|_| unreachable(NO_ANSWER))?;
    match response.status().as_u16() {
        200 => {}
        401 | 403 => {
            return Err(ApiError::invalid_input(
                "Deepgram rejected this API key (invalid credentials)",
            )
            .with_hint(
                "create a key in the Deepgram console (Member role or higher) and paste it exactly",
            )
            .with_details(json!({"reason": "key_invalid"})));
        }
        status => return Err(unreachable(&format!("HTTP {status} from /v1/auth/token"))),
    }
    match grant(state, base_url, key, 1).await {
        // A key that may not mint JWTs still works: peekd then goes through
        // the speech proxy (`{"mode":"proxy"}`), as with peek's own key.
        Ok(_) | Err(Failure::Status { status: 403, .. }) => Ok(()),
        Err(Failure::Status { status: 402, .. }) => Err(ApiError::invalid_input(
            "this Deepgram project has no credits left",
        )
        .with_details(json!({"reason": "out_of_credits"}))),
        Err(Failure::Status { status: 401, .. }) => Err(ApiError::invalid_input(
            "Deepgram rejected this API key when minting a token",
        )
        .with_details(json!({"reason": "key_invalid"}))),
        Err(Failure::Status { status, .. }) => {
            Err(unreachable(&format!("HTTP {status} from /v1/auth/grant")))
        }
        Err(Failure::Transport(_)) => Err(unreachable(NO_ANSWER)),
        Err(Failure::Malformed) => Err(unreachable("an unexpected /v1/auth/grant answer")),
    }
}

/// Maps a minting failure for `org` to `503 speech_unavailable {reason}`.
pub(crate) fn mint_error(source: KeySource, org: &str, failure: &Failure) -> ApiError {
    unavailable(source, org, failure, "when minting a token")
}

/// The hint for a broken org key: the exact commands that fix or remove it.
pub(crate) fn org_key_hint(org: &str) -> String {
    format!(
        "an org owner or admin must fix or remove the key: peek --org {org} org byo deepgram set --key-file - \
(or: peek --org {org} org byo deepgram delete); peek never falls back to its own key"
    )
}

/// `503 speech_unavailable {reason, key_source, dg_request_id?}` for a key
/// problem or an outage; `during` says which call failed.
fn unavailable(source: KeySource, org: &str, failure: &Failure, during: &str) -> ApiError {
    let who = match source {
        KeySource::Org => "the org's own Deepgram key",
        KeySource::Peek => "peek's Deepgram key",
    };
    let (reason, message, retry): (&str, String, Option<u64>) = match (source, failure) {
        (KeySource::Org, Failure::Status { status: 401, .. }) => (
            "org_key_invalid",
            format!("Deepgram rejected {who} {during}"),
            None,
        ),
        (KeySource::Org, Failure::Status { status: 402, .. }) => (
            "org_out_of_credits",
            format!("the Deepgram project behind {who} has no credits"),
            None,
        ),
        (KeySource::Org, Failure::Status { status: 403, .. }) => (
            "org_model_forbidden",
            format!(
                "{who} is not allowed to do this {during} (it needs the Member role, or access to the model)"
            ),
            None,
        ),
        (
            KeySource::Peek,
            Failure::Status {
                status: 401 | 403, ..
            },
        ) => (
            "peek_key_invalid",
            format!("Deepgram rejected {who} {during}"),
            None,
        ),
        (KeySource::Peek, Failure::Status { status: 402, .. }) => (
            "peek_out_of_credits",
            format!("the Deepgram project behind {who} has no credits"),
            None,
        ),
        (
            _,
            Failure::Status {
                status: 429,
                retry_after,
                ..
            },
        ) => (
            "rate_limited",
            format!("Deepgram is rate limiting {who}"),
            Some(retry_after.unwrap_or(5).clamp(1, 60)),
        ),
        (_, Failure::Status { status, .. }) => (
            "deepgram_unavailable",
            format!("Deepgram answered HTTP {status} {during} with {who}"),
            Some(5),
        ),
        (_, Failure::Transport(why)) => (
            "deepgram_unavailable",
            format!("peek-server could not reach Deepgram {during}: {why}"),
            Some(5),
        ),
        (_, Failure::Malformed) => (
            "deepgram_unavailable",
            format!("Deepgram answered {during} with an unexpected body"),
            Some(5),
        ),
    };
    let hint = match source {
        KeySource::Org => org_key_hint(org),
        KeySource::Peek => {
            "speech is degraded to text until the operator fixes peek's Deepgram key; retry later"
                .to_owned()
        }
    };
    let mut details = json!({"reason": reason, "key_source": source});
    if let Failure::Status {
        request_id: Some(id),
        ..
    } = failure
    {
        details["dg_request_id"] = json!(id);
    }
    let error = ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::SpeechUnavailable,
        message,
    )
    .with_hint(hint)
    .with_details(details);
    match retry {
        Some(seconds) => error.with_retry_after(seconds),
        None => error.with_retryable(false),
    }
}

/// Maps a failed proxied `/v1/speak` or `/v1/listen` call. Key problems
/// and outages are `503 speech_unavailable {reason}` (as for minting); a
/// request Deepgram refuses (400, 413, 415, 422…) is `400 invalid_input`
/// with `details.reason = "deepgram_rejected"`; `422` is retryable once
/// (an interrupted upload).
pub(crate) fn proxy_error(
    source: KeySource,
    org: &str,
    route: &str,
    failure: &Failure,
) -> ApiError {
    let rejected = |status: u16, detail: &Option<String>, request_id: &Option<String>| {
        let e = ApiError::new(
            StatusCode::BAD_REQUEST,
            ErrorCode::InvalidInput,
            format!(
                "Deepgram refused {route} with HTTP {status}{}",
                detail
                    .as_deref()
                    .map_or_else(String::new, |d| format!(" ({d})"))
            ),
        )
        .with_hint("check the model, text length, audio format and parameters")
        .with_details(json!({"reason": "deepgram_rejected", "deepgram_status": status, "dg_request_id": request_id}));
        if status == 422 {
            e.with_retryable(true)
        } else {
            e
        }
    };
    match failure {
        Failure::Status {
            status: status @ (400 | 404 | 405 | 411 | 413 | 415 | 422),
            detail,
            request_id,
            ..
        } => rejected(*status, detail, request_id),
        other => unavailable(source, org, other, &format!("to {route}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byo_failures_have_distinct_reasons_and_never_retry() {
        for (status, reason) in [
            (401, "org_key_invalid"),
            (402, "org_out_of_credits"),
            (403, "org_model_forbidden"),
        ] {
            let e = mint_error(
                KeySource::Org,
                "tos",
                &Failure::Status {
                    status,
                    retry_after: None,
                    detail: None,
                    request_id: None,
                },
            );
            assert_eq!(*e.code(), ErrorCode::SpeechUnavailable);
            assert_eq!(
                e.details().map(|d| d["reason"].clone()),
                Some(json!(reason))
            );
            assert!(e.retry_after().is_none());
        }
        let e = mint_error(KeySource::Peek, "tos", &Failure::Transport("timed out"));
        assert_eq!(e.retry_after(), Some(5));
    }

    #[test]
    fn proxy_failures() {
        let status = |status| Failure::Status {
            status,
            retry_after: None,
            detail: Some("INVALID_MODEL: no such model".into()),
            request_id: Some("dg-1".into()),
        };
        let e = proxy_error(KeySource::Peek, "tos", "/v1/speak", &status(400));
        assert_eq!(*e.code(), ErrorCode::InvalidInput);
        assert!(e.message().contains("INVALID_MODEL"));
        assert_eq!(
            e.details().map(|d| d["reason"].clone()),
            Some(json!("deepgram_rejected"))
        );
        let e = proxy_error(KeySource::Peek, "tos", "/v1/speak", &status(401));
        assert_eq!(*e.code(), ErrorCode::SpeechUnavailable);
        assert_eq!(
            e.details().map(|d| d["reason"].clone()),
            Some(json!("peek_key_invalid"))
        );
        assert_eq!(
            e.details().map(|d| d["dg_request_id"].clone()),
            Some(json!("dg-1"))
        );
        let e = proxy_error(KeySource::Org, "tos", "/v1/listen", &status(402));
        assert_eq!(
            e.details().map(|d| d["reason"].clone()),
            Some(json!("org_out_of_credits"))
        );
        let e = proxy_error(KeySource::Peek, "tos", "/v1/listen", &status(503));
        assert_eq!(e.retry_after(), Some(5));
        assert_eq!(
            common_query(&["peek".into(), "production".into(), "peek".into()]),
            vec![
                ("tag".to_owned(), "peek".to_owned()),
                ("tag".to_owned(), "production".to_owned()),
                ("mip_opt_out".to_owned(), "true".to_owned())
            ]
        );
        let k = Secret::new("dg-key-0123456789");
        assert_eq!(
            key_fingerprint("https://a", &k),
            key_fingerprint("https://a", &k)
        );
        assert_ne!(
            key_fingerprint("https://a", &k),
            key_fingerprint("https://b", &k)
        );
    }
}
