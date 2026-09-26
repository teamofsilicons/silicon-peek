//! Deepgram over plain `reqwest` (BLUEPRINT §8.7, notes/speech §4.1): the
//! streamed Aura-2 `POST /v1/speak` and the pre-recorded Nova-3
//! `POST /v1/listen`.
//!
//! In direct mode both authenticate with the ≤ 60 s JWT that peek-server
//! mints (`Authorization: Bearer <jwt>`); the base URL comes from the token
//! response, so tests point it at a mock. JWTs are never logged. In proxy
//! mode (the server's key cannot mint JWTs) the same requests go through
//! peek-server's speech proxy with the Silicon's session instead; see
//! `speech.rs`. Every request carries `mip_opt_out=true`.

use std::time::Duration;

use serde::Deserialize;
use silicon_peek_client::{
    Error, ErrorCode, Result,
    api::{SpeechMode, SpeechToken},
    error::Origin,
};
use url::Url;

/// Longest `Retry-After` honoured before a TTS or STT retry (§5.3: ~2 s).
pub const RETRY_AFTER_CAP: Duration = Duration::from_secs(2);

/// The most of an error body peekd reads: Deepgram's envelope is a few
/// hundred bytes. The base URL can be an org's own endpoint, so no body is
/// trusted to be small.
const ERROR_BODY_MAX: usize = 8 * 1024;
/// How long peekd waits for an error body (the client has no overall
/// timeout; the status alone already classifies the failure).
const ERROR_BODY_WAIT: Duration = Duration::from_secs(5);
/// The most of a transcription's JSON peekd reads (Deepgram's answer for a
/// 4 MiB recording is far smaller).
const TRANSCRIPT_MAX: usize = 16 * 1024 * 1024;

/// Why a capped read stopped.
enum Capped {
    /// The body is (or announced itself as) bigger than the cap.
    TooLarge,
    /// The connection failed while reading.
    Failed(reqwest::Error),
}

/// Reads at most `max` bytes of a body, chunk by chunk, stopping (and
/// dropping the connection) as soon as it announces or passes `max`.
async fn read_capped(
    mut resp: reqwest::Response,
    max: usize,
) -> std::result::Result<Vec<u8>, Capped> {
    let limit = u64::try_from(max).unwrap_or(u64::MAX);
    if resp.content_length().is_some_and(|n| n > limit) {
        return Err(Capped::TooLarge);
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(Capped::Failed)? {
        if body.len().saturating_add(chunk.len()) > max {
            return Err(Capped::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// An error body: at most [`ERROR_BODY_MAX`] bytes within
/// [`ERROR_BODY_WAIT`]; a bigger, slower or broken body is dropped.
async fn error_body(resp: reqwest::Response) -> Vec<u8> {
    match tokio::time::timeout(ERROR_BODY_WAIT, read_capped(resp, ERROR_BODY_MAX)).await {
        Ok(Ok(body)) => body,
        _ => Vec::new(),
    }
}

/// How a failed Deepgram call may be retried.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DgKind {
    /// Transport, 408, 429, 5xx: retry with backoff.
    Retry,
    /// 422 (interrupted upload): retry once.
    RetryOnce,
    /// 401: mint a new JWT once and retry once.
    Reauth,
    /// 400, 402, 403, 413, 415 and anything else: do not retry.
    Fatal,
}

/// A failed Deepgram call.
#[derive(Debug)]
pub struct DgError {
    /// Retry class.
    pub kind: DgKind,
    /// The error to report.
    pub error: Error,
    /// `Retry-After`, capped.
    pub retry_after: Option<Duration>,
}

impl DgError {
    fn fatal(error: Error) -> Self {
        Self {
            kind: DgKind::Fatal,
            error,
            retry_after: None,
        }
    }
}

/// The §5.3 retry budget of one TTS or STT call: a fixed list of delays,
/// one retry on 422, one JWT re-mint on 401, all inside a deadline.
#[derive(Clone, Debug)]
pub struct RetryBudget<'a> {
    deadline: std::time::Instant,
    delays: &'a [Duration],
    used: usize,
    reauthed: bool,
    retried_422: bool,
}

impl<'a> RetryBudget<'a> {
    /// A budget ending at `deadline` with `delays`.
    #[must_use]
    pub fn new(deadline: std::time::Instant, delays: &'a [Duration]) -> Self {
        Self {
            deadline,
            delays,
            used: 0,
            reauthed: false,
            retried_422: false,
        }
    }

    /// The delay before the next retry of a failure of `kind`, if any is left.
    pub fn next(&mut self, kind: DgKind) -> Option<Duration> {
        let allowed = match kind {
            DgKind::Retry => true,
            DgKind::RetryOnce => !std::mem::replace(&mut self.retried_422, true),
            DgKind::Reauth | DgKind::Fatal => false,
        };
        let delay = *self.delays.get(self.used)?;
        if !allowed || std::time::Instant::now() + delay >= self.deadline {
            return None;
        }
        self.used += 1;
        Some(delay)
    }

    /// Claims the single JWT re-mint; false when it was already used.
    pub fn reauth(&mut self) -> bool {
        !std::mem::replace(&mut self.reauthed, true)
    }

    /// Time left before the deadline.
    #[must_use]
    pub fn remaining(&self) -> Duration {
        self.deadline
            .saturating_duration_since(std::time::Instant::now())
    }
}

/// STT language selection (notes/speech §6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SttLanguage {
    /// `language=<l>`.
    Fixed(String),
    /// Repeated `detect_language=<a>`.
    Detect(Vec<String>),
    /// `detect_language=true`.
    DetectAny,
}

/// Query parameters of one transcription.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListenParams {
    /// `numerals=true` (slider and range asks).
    pub numerals: bool,
    /// One `keyterm` per option label (≤ 500 tokens in total).
    pub keyterms: Vec<String>,
    /// Language handling.
    pub language: SttLanguage,
}

/// A transcription.
#[derive(Clone, Debug, PartialEq)]
pub struct Transcript {
    /// `results.channels[0].alternatives[0].transcript`, trimmed.
    pub text: String,
    /// `metadata.request_id`.
    pub request_id: Option<String>,
    /// `results.channels[0].detected_language`.
    pub detected_language: Option<String>,
}

#[derive(Deserialize)]
struct ListenBody {
    #[serde(default)]
    metadata: Option<ListenMeta>,
    results: ListenResults,
}

#[derive(Deserialize)]
struct ListenMeta {
    #[serde(default)]
    request_id: Option<String>,
}

#[derive(Deserialize)]
struct ListenResults {
    channels: Vec<ListenChannel>,
}

#[derive(Deserialize)]
struct ListenChannel {
    #[serde(default)]
    detected_language: Option<String>,
    alternatives: Vec<ListenAlternative>,
}

#[derive(Deserialize)]
struct ListenAlternative {
    transcript: String,
}

/// The Deepgram HTTP client (a warm, pooled HTTP/2 connection).
#[derive(Clone, Debug)]
pub struct Deepgram {
    http: reqwest::Client,
}

/// The endpoint `path` under a token's base URL: HTTPS, or HTTP to loopback
/// only (audio and JWTs never cross a network in the clear).
///
/// # Errors
/// `speech_unavailable` for any other base URL.
pub fn endpoint(base: &str, path: &str) -> Result<Url> {
    let url = Url::parse(base).map_err(|e| bad_base(base, &e.to_string()))?;
    let loopback = matches!(
        url.host_str(),
        Some("127.0.0.1" | "localhost" | "[::1]" | "::1")
    );
    if !(url.scheme() == "https" || (url.scheme() == "http" && loopback)) {
        return Err(bad_base(
            base,
            "only https (or http to loopback) is allowed",
        ));
    }
    url.join(path).map_err(|e| bad_base(base, &e.to_string()))
}

fn bad_base(base: &str, why: &str) -> Error {
    Error::new(
        ErrorCode::SpeechUnavailable,
        format!("peek-server returned an unusable Deepgram base URL `{base}`: {why}"),
    )
    .with_hint("an org admin can fix the BYO base URL with `peek org byo deepgram set`")
}

fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s >= 0.0)
        .map(|s| Duration::from_secs_f64(s).min(RETRY_AFTER_CAP))
}

fn request_id(headers: &reqwest::header::HeaderMap) -> Option<String> {
    headers
        .get("dg-request-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}

/// Decodes a Deepgram error body (both envelope shapes, leniently).
fn describe_error(body: &[u8]) -> String {
    let v: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
    let code = ["err_code", "category", "error_code"]
        .iter()
        .find_map(|k| v.get(*k).and_then(|x| x.as_str()));
    let msg = ["err_msg", "message", "details"]
        .iter()
        .find_map(|k| v.get(*k).and_then(|x| x.as_str()));
    match (code, msg) {
        (Some(c), Some(m)) => format!("{c}: {m}"),
        (Some(c), None) => c.to_owned(),
        (None, Some(m)) => m.to_owned(),
        (None, None) => String::from_utf8_lossy(&body[..body.len().min(200)])
            .trim()
            .to_owned(),
    }
}

fn status_error(
    route: &str,
    status: u16,
    headers: &reqwest::header::HeaderMap,
    body: &[u8],
) -> DgError {
    let detail = describe_error(body);
    let rid = request_id(headers);
    let (kind, reason, hint) = match status {
        401 => (
            DgKind::Reauth,
            "unauthorized",
            "the Deepgram key or token was rejected; if the org uses its own key, an admin must replace it",
        ),
        402 => (
            DgKind::Fatal,
            "out_of_credits",
            "the Deepgram project is out of credits",
        ),
        403 => (
            DgKind::Fatal,
            "forbidden",
            "the Deepgram key may not use this model",
        ),
        408 | 429 | 500..=599 => (
            DgKind::Retry,
            "unavailable",
            "Deepgram is busy or down; peek retries briefly",
        ),
        422 => (
            DgKind::RetryOnce,
            "unprocessable",
            "the upload was interrupted",
        ),
        _ => (
            DgKind::Fatal,
            "rejected",
            "this looks like a peek bug; run peek report",
        ),
    };
    DgError {
        kind,
        error: Error::new(
            ErrorCode::SpeechUnavailable,
            format!("Deepgram answered {route} with HTTP {status} ({detail})"),
        )
        .with_hint(hint)
        .with_retryable(matches!(kind, DgKind::Retry | DgKind::RetryOnce))
        .with_status(status)
        .with_request_id(rid)
        .with_details(serde_json::json!({"reason": reason}))
        .with_origin(Origin::Server),
        retry_after: retry_after(headers),
    }
}

fn transport_error(route: &str, e: &reqwest::Error) -> DgError {
    DgError {
        kind: DgKind::Retry,
        error: Error::new(
            ErrorCode::SpeechUnavailable,
            format!("the request to Deepgram {route} failed: {e}"),
        )
        .with_hint("check the network; peek retries briefly")
        .with_retryable(true)
        .with_origin(Origin::Transport),
        retry_after: None,
    }
}

/// `tag=peek` plus the token's tags, then `mip_opt_out=true` — always: peek
/// opts out of Deepgram's Model Improvement Program whatever the token says.
fn add_common(url: &mut Url, token: &SpeechToken) {
    let mut q = url.query_pairs_mut();
    let mut tags: Vec<&str> = vec!["peek"];
    for t in &token.params.tags {
        if !tags.contains(&t.as_str()) {
            tags.push(t);
        }
    }
    for t in tags {
        q.append_pair("tag", t);
    }
    q.append_pair("mip_opt_out", "true");
}

/// The JWT of a direct-mode token.
fn jwt(token: &SpeechToken) -> std::result::Result<&str, DgError> {
    match (token.mode, token.access_token.as_ref()) {
        (SpeechMode::Direct, Some(t)) => Ok(t.expose()),
        _ => Err(DgError::fatal(Error::new(
            ErrorCode::SpeechUnavailable,
            "peek-server's speech token carries no Deepgram JWT for a direct call",
        ))),
    }
}

/// The Nova-3 query parameters of a transcription (without tags and
/// `mip_opt_out`, which the caller or the speech proxy adds).
#[must_use]
pub fn listen_query(params: &ListenParams) -> Vec<(String, String)> {
    let mut q = vec![
        ("model".to_owned(), "nova-3".to_owned()),
        ("smart_format".to_owned(), "true".to_owned()),
    ];
    if params.numerals {
        q.push(("numerals".to_owned(), "true".to_owned()));
    }
    for k in &params.keyterms {
        q.push(("keyterm".to_owned(), k.clone()));
    }
    match &params.language {
        SttLanguage::Fixed(l) => q.push(("language".to_owned(), l.clone())),
        SttLanguage::Detect(ls) => {
            for l in ls {
                q.push(("detect_language".to_owned(), l.clone()));
            }
        }
        SttLanguage::DetectAny => q.push(("detect_language".to_owned(), "true".to_owned())),
    }
    q
}

/// The Aura-2 query for `model` (always explicit: notes/speech §9 doc drift).
///
/// # Errors
/// As [`endpoint`].
pub fn speak_url(token: &SpeechToken, model: &str) -> Result<Url> {
    let mut url = endpoint(&token.base_url, "/v1/speak")?;
    url.query_pairs_mut()
        .append_pair("model", model)
        .append_pair("encoding", "linear16")
        .append_pair("container", "none")
        .append_pair("sample_rate", "24000");
    add_common(&mut url, token);
    Ok(url)
}

/// The Nova-3 query.
///
/// # Errors
/// As [`endpoint`].
pub fn listen_url(token: &SpeechToken, params: &ListenParams) -> Result<Url> {
    let mut url = endpoint(&token.base_url, "/v1/listen")?;
    url.query_pairs_mut().extend_pairs(listen_query(params));
    add_common(&mut url, token);
    Ok(url)
}

/// Decodes a Nova-3 result: the first channel's first alternative, trimmed.
///
/// # Errors
/// `speech_unavailable` for an unreadable body.
pub fn parse_transcript(
    body: &[u8],
    header_request_id: Option<String>,
) -> std::result::Result<Transcript, DgError> {
    let parsed: ListenBody = serde_json::from_slice(body).map_err(|e| {
        DgError::fatal(Error::new(
            ErrorCode::SpeechUnavailable,
            format!("Deepgram /v1/listen returned an unreadable result: {e}"),
        ))
    })?;
    let channel = parsed.results.channels.into_iter().next();
    let detected_language = channel.as_ref().and_then(|c| c.detected_language.clone());
    let text = channel
        .and_then(|c| c.alternatives.into_iter().next())
        .map(|a| a.transcript.trim().to_owned())
        .unwrap_or_default();
    Ok(Transcript {
        text,
        request_id: parsed
            .metadata
            .and_then(|m| m.request_id)
            .or(header_request_id),
        detected_language,
    })
}

/// Whether a `Content-Type` is raw linear16 audio.
#[must_use]
pub fn is_linear16(content_type: &str) -> bool {
    let c = content_type.to_ascii_lowercase();
    c.starts_with("audio/l16") || c.starts_with("audio/pcm")
}

/// Classifies a failed call to peek-server's speech proxy like a failed
/// Deepgram call: transport, 5xx, 429 and retryable `speech_unavailable`
/// retry; `401 unauthenticated` refreshes the session once; a retryable 4xx
/// (Deepgram's 422) retries once; everything else is final.
#[must_use]
pub fn classify_proxy_error(e: Error) -> DgError {
    let retry_after = e.retry_after().map(|d| d.min(RETRY_AFTER_CAP));
    let kind = if e.is_transport() {
        DgKind::Retry
    } else {
        match e.status() {
            Some(401) => DgKind::Reauth,
            Some(400 | 422) if e.retryable() => DgKind::RetryOnce,
            _ if e.retryable() => DgKind::Retry,
            _ => DgKind::Fatal,
        }
    };
    DgError {
        kind,
        error: e,
        retry_after,
    }
}

/// Keyterms from option labels, one per term, capped at 500 tokens in total
/// (notes/speech §2.1).
#[must_use]
pub fn keyterms<'a>(labels: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut tokens = 0usize;
    for l in labels {
        let l = l.trim();
        if l.is_empty() || out.iter().any(|o| o == l) {
            continue;
        }
        let n = l.split_whitespace().count().max(1);
        if tokens + n > 500 {
            break;
        }
        tokens += n;
        out.push(l.to_owned());
    }
    out
}

impl Deepgram {
    /// A pooled client: HTTP/2 when offered, 5 s connect timeout, no overall
    /// timeout (TTS bodies stream; callers bound each phase).
    ///
    /// # Errors
    /// `internal_error` if TLS cannot initialize.
    pub fn new() -> Result<Self> {
        silicon_peek_client::http::ensure_crypto_provider();
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .pool_idle_timeout(Duration::from_secs(90))
            .tcp_keepalive(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("peekd/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| {
                Error::internal(format!("could not initialize the Deepgram client: {e}"))
            })?;
        Ok(Self { http })
    }

    /// Starts `POST /v1/speak`; returns the response once its status and
    /// content type say the body is raw 24 kHz s16le audio.
    ///
    /// # Errors
    /// A classified [`DgError`].
    pub async fn speak(
        &self,
        token: &SpeechToken,
        model: &str,
        text: &str,
        first_byte: Duration,
    ) -> std::result::Result<(reqwest::Response, Option<String>), DgError> {
        let url = speak_url(token, model).map_err(DgError::fatal)?;
        let bearer = jwt(token)?;
        let send = self
            .http
            .post(url)
            .bearer_auth(bearer)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .json(&serde_json::json!({ "text": text }))
            .send();
        let resp = match tokio::time::timeout(first_byte, send).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => return Err(transport_error("/v1/speak", &e)),
            Err(_) => {
                return Err(DgError {
                    kind: DgKind::Retry,
                    error: Error::new(
                        ErrorCode::SpeechUnavailable,
                        format!(
                            "Deepgram /v1/speak did not answer within {} ms",
                            first_byte.as_millis()
                        ),
                    )
                    .with_retryable(true)
                    .with_origin(Origin::Transport),
                    retry_after: None,
                });
            }
        };
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        if status != 200 {
            let body = error_body(resp).await;
            return Err(status_error("/v1/speak", status, &headers, &body));
        }
        let ctype = headers
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !is_linear16(&ctype) {
            let body = error_body(resp).await;
            return Err(DgError::fatal(
                Error::new(
                    ErrorCode::SpeechUnavailable,
                    format!(
                        "Deepgram /v1/speak returned `{ctype}` instead of raw linear16 audio ({})",
                        describe_error(&body)
                    ),
                )
                .with_request_id(request_id(&headers)),
            ));
        }
        Ok((resp, request_id(&headers)))
    }

    /// `POST /v1/listen` with a WAV body.
    ///
    /// # Errors
    /// A classified [`DgError`].
    pub async fn listen(
        &self,
        token: &SpeechToken,
        params: &ListenParams,
        wav: Vec<u8>,
        timeout: Duration,
    ) -> std::result::Result<Transcript, DgError> {
        let url = listen_url(token, params).map_err(DgError::fatal)?;
        let bearer = jwt(token)?;
        let send = self
            .http
            .post(url)
            .bearer_auth(bearer)
            .header(reqwest::header::CONTENT_TYPE, "audio/wav")
            .timeout(timeout)
            .body(wav)
            .send();
        let resp = send.await.map_err(|e| transport_error("/v1/listen", &e))?;
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        if status != 200 {
            let body = error_body(resp).await;
            return Err(status_error("/v1/listen", status, &headers, &body));
        }
        let body = match read_capped(resp, TRANSCRIPT_MAX).await {
            Ok(body) => body,
            Err(Capped::Failed(e)) => return Err(transport_error("/v1/listen", &e)),
            Err(Capped::TooLarge) => {
                return Err(DgError::fatal(
                    Error::new(
                        ErrorCode::SpeechUnavailable,
                        format!(
                            "Deepgram /v1/listen answered with more than {} MiB of JSON",
                            TRANSCRIPT_MAX / (1024 * 1024)
                        ),
                    )
                    .with_request_id(request_id(&headers))
                    .with_details(serde_json::json!({"reason": "malformed"})),
                ));
            }
        };
        parse_transcript(&body, request_id(&headers))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use silicon_peek_client::{
        Secret,
        api::{KeySource, SpeechParams},
    };

    fn token(base: &str, mip: bool) -> SpeechToken {
        SpeechToken {
            mode: SpeechMode::Direct,
            access_token: Some(Secret::new("jwt")),
            expires_in: 60,
            base_url: base.to_owned(),
            key_source: KeySource::Peek,
            params: SpeechParams {
                mip_opt_out: mip,
                tags: vec!["peek".into(), "production".into()],
            },
        }
    }

    #[test]
    fn urls_are_explicit() -> Result<()> {
        let t = token("https://api.deepgram.com", true);
        let u = speak_url(&t, "aura-2-thalia-en")?;
        assert_eq!(
            u.as_str(),
            "https://api.deepgram.com/v1/speak?model=aura-2-thalia-en&encoding=linear16&container=none&sample_rate=24000&tag=peek&tag=production&mip_opt_out=true"
        );
        let p = ListenParams {
            numerals: true,
            keyterms: vec!["Keep it".into(), "Delete".into()],
            language: SttLanguage::Detect(vec!["en".into(), "hi".into()]),
        };
        // mip_opt_out=true is sent even when a token says otherwise.
        let u = listen_url(&token("http://127.0.0.1:9", false), &p)?;
        assert_eq!(
            u.as_str(),
            "http://127.0.0.1:9/v1/listen?model=nova-3&smart_format=true&numerals=true&keyterm=Keep+it&keyterm=Delete&detect_language=en&detect_language=hi&tag=peek&tag=production&mip_opt_out=true"
        );
        assert!(endpoint("http://api.deepgram.com", "/v1/speak").is_err());
        assert!(endpoint("ftp://x", "/v1/speak").is_err());
        Ok(())
    }

    #[test]
    fn keyterm_budget() {
        let long = "word ".repeat(300);
        let k = keyterms(["Keep", "Keep", " ", long.as_str(), long.as_str(), "Last"]);
        assert_eq!(
            k.len(),
            3,
            "duplicate, blank and over-budget terms are skipped"
        );
        assert_eq!(k[0], "Keep");
        assert_eq!(k[2], "Last");
    }

    /// A raw HTTP/1.1 server answering every request with `status_line`
    /// and a chunked body that never ends; returns its base URL and a
    /// counter of the bytes it wrote.
    async fn endless(
        status_line: &'static str,
    ) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::{Arc, atomic::AtomicUsize, atomic::Ordering};
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let addr = listener.local_addr().unwrap_or_else(|e| panic!("{e}"));
        let written = Arc::new(AtomicUsize::new(0));
        let counter = written.clone();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let counter = counter.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 64 * 1024];
                    let mut seen = Vec::new();
                    loop {
                        match sock.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => seen.extend_from_slice(&buf[..n]),
                        }
                        let text = String::from_utf8_lossy(&seen).to_ascii_lowercase();
                        if let Some(end) = text.find("\r\n\r\n") {
                            let len = text
                                .lines()
                                .find_map(|l| l.strip_prefix("content-length:"))
                                .and_then(|v| v.trim().parse::<usize>().ok())
                                .unwrap_or(0);
                            if seen.len() >= end + 4 + len {
                                break;
                            }
                        }
                    }
                    let head = format!(
                        "{status_line}\r\ncontent-type: application/json\r\ntransfer-encoding: chunked\r\n\r\n"
                    );
                    if sock.write_all(head.as_bytes()).await.is_err() {
                        return;
                    }
                    let mut chunk = format!("{:x}\r\n", 64 * 1024).into_bytes();
                    chunk.extend_from_slice(&vec![b'x'; 64 * 1024]);
                    chunk.extend_from_slice(b"\r\n");
                    for _ in 0..8192 {
                        if sock.write_all(&chunk).await.is_err() {
                            return;
                        }
                        counter.fetch_add(chunk.len(), Ordering::Relaxed);
                    }
                });
            }
        });
        (format!("http://{addr}"), written)
    }

    #[tokio::test]
    async fn endless_bodies_are_capped() -> Result<()> {
        use std::sync::atomic::Ordering;
        let dg = Deepgram::new()?;
        let params = ListenParams {
            numerals: false,
            keyterms: Vec::new(),
            language: SttLanguage::DetectAny,
        };
        let (base, written) = endless("HTTP/1.1 500 Internal Server Error").await;
        let started = std::time::Instant::now();
        let Err(e) = dg
            .speak(
                &token(&base, true),
                "aura-2-thalia-en",
                "hi",
                Duration::from_secs(5),
            )
            .await
        else {
            panic!("a 500 must fail");
        };
        assert_eq!(e.kind, DgKind::Retry);
        let Err(e) = dg
            .listen(
                &token(&base, true),
                &params,
                vec![0; 64],
                Duration::from_secs(60),
            )
            .await
        else {
            panic!("a 500 must fail");
        };
        assert_eq!(e.kind, DgKind::Retry);
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "error bodies are read within a bounded time"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(written.load(Ordering::Relaxed) < 64 * 1024 * 1024);

        let (base, written) = endless("HTTP/1.1 200 OK").await;
        let Err(e) = dg
            .listen(
                &token(&base, true),
                &params,
                vec![0; 64],
                Duration::from_secs(60),
            )
            .await
        else {
            panic!("an endless transcription must fail");
        };
        assert_eq!(e.kind, DgKind::Fatal);
        assert!(e.error.message().contains("more than 16 MiB"));
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(written.load(Ordering::Relaxed) < 96 * 1024 * 1024);
        Ok(())
    }

    #[test]
    fn error_classes() {
        let h = reqwest::header::HeaderMap::new();
        assert_eq!(
            status_error("/v1/speak", 401, &h, b"{}").kind,
            DgKind::Reauth
        );
        assert_eq!(status_error("/v1/speak", 503, &h, b"").kind, DgKind::Retry);
        assert_eq!(
            status_error("/v1/speak", 422, &h, b"").kind,
            DgKind::RetryOnce
        );
        let e = status_error("/v1/listen", 402, &h, br#"{"err_code":"ASR_PAYMENT_REQUIRED","err_msg":"Project does not have enough credits"}"#);
        assert_eq!(e.kind, DgKind::Fatal);
        assert!(
            e.error
                .message()
                .contains("ASR_PAYMENT_REQUIRED: Project does not have enough credits")
        );
        assert_eq!(
            e.error.details().map(|d| d["reason"].clone()),
            Some(serde_json::json!("out_of_credits"))
        );
        let mut h = reqwest::header::HeaderMap::new();
        h.insert(
            reqwest::header::RETRY_AFTER,
            reqwest::header::HeaderValue::from_static("30"),
        );
        assert_eq!(
            status_error("/v1/speak", 429, &h, b"").retry_after,
            Some(RETRY_AFTER_CAP)
        );
    }
}
