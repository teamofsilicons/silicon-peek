//! Shared speech retry rules and transcription hints for Peek's authenticated proxy.

use std::time::Duration;

use silicon_peek_client::Error;

/// Longest `Retry-After` honoured before a TTS or STT retry.
pub const RETRY_AFTER_CAP: Duration = Duration::from_secs(2);

/// How a failed speech request may be retried.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetryKind {
    /// Transport, 408, 429, 5xx: retry with backoff.
    Retry,
    /// 422 (interrupted upload): retry once.
    RetryOnce,
    /// 401: refresh the Peek session once and retry once.
    Reauth,
    /// 400, 402, 403, 413, 415 and anything else: do not retry.
    Fatal,
}

/// A failed speech request.
#[derive(Debug)]
pub struct SpeechError {
    /// Retry class.
    pub kind: RetryKind,
    /// The error to report.
    pub error: Error,
    /// `Retry-After`, capped.
    pub retry_after: Option<Duration>,
}

/// The §5.3 retry budget of one TTS or STT call: a fixed list of delays,
/// one retry on 422, one session refresh on 401, all inside a deadline.
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
    pub fn next(&mut self, kind: RetryKind) -> Option<Duration> {
        let allowed = match kind {
            RetryKind::Retry => true,
            RetryKind::RetryOnce => !std::mem::replace(&mut self.retried_422, true),
            RetryKind::Reauth | RetryKind::Fatal => false,
        };
        let delay = *self.delays.get(self.used)?;
        if !allowed || std::time::Instant::now() + delay >= self.deadline {
            return None;
        }
        self.used += 1;
        Some(delay)
    }

    /// Claims the single session refresh; false when it was already used.
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
    /// One `keyterm` per option label (at most 100 bounded terms).
    pub keyterms: Vec<String>,
    /// Language handling.
    pub language: SttLanguage,
}

/// Transcription hints sent to Peek; provider credentials remain on the server.
#[must_use]
pub fn listen_query(params: &ListenParams) -> Vec<(String, String)> {
    let mut q = vec![("model".to_owned(), "gpt-transcribe".to_owned())];
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

/// Classifies a failed call to Peek: transport, 5xx, 429 and retryable `speech_unavailable`
/// retry; `401 unauthenticated` refreshes the session once; a retryable 4xx
/// (422) retries once; everything else is final.
#[must_use]
pub fn classify_proxy_error(e: Error) -> SpeechError {
    let retry_after = e.retry_after().map(|d| d.min(RETRY_AFTER_CAP));
    let kind = if e.is_transport() {
        RetryKind::Retry
    } else {
        match e.status() {
            Some(401) => RetryKind::Reauth,
            Some(400 | 422) if e.retryable() => RetryKind::RetryOnce,
            _ if e.retryable() => RetryKind::Retry,
            _ => RetryKind::Fatal,
        }
    };
    SpeechError {
        kind,
        error: e,
        retry_after,
    }
}

/// Safe `OpenAI` keyword hints from option labels, deduplicated and bounded.
#[must_use]
pub fn keyterms<'a>(labels: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut out = Vec::new();
    for label in labels {
        if label.contains(['<', '>']) || label.chars().any(char::is_control) {
            continue;
        }
        let label = label.trim();
        if (1..=100).contains(&label.chars().count()) && !out.iter().any(|v| v == label) {
            out.push(label.to_owned());
            if out.len() == 100 {
                break;
            }
        }
    }
    out
}
