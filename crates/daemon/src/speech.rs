//! Text to speech (BLUEPRINT §1.9.3, §8.7): the speech-token cache, the TTS
//! cache (`cache/tts/<sha256>.pcm`, LRU 200 MB / 30 days) and the streaming
//! task that drains Deepgram at network speed and forwards `tts.begin` /
//! `tts.chunk` / `tts.end` to Peek.app. Retries happen only before the first
//! audio byte; once audio has reached the UI, a failure ends the stream early
//! instead of repeating speech.
//!
//! The token decides how Deepgram is reached. `direct`: peekd calls
//! Deepgram with the minted JWT. `proxy` (peek-server's key cannot mint
//! JWTs): peekd calls peek-server's `/api/v1/speech/speak|listen` with the
//! Silicon's own session — only when the proxy `base_url` is that home's
//! backend, so a session never goes anywhere else. The token (and so the
//! mode) is cached until it expires.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant, SystemTime},
};

use serde_json::json;
use silicon_peek_client::{
    Error, ErrorCode, Result,
    api::{SpeechMode, SpeechPurpose, SpeechSpeakRequest, SpeechToken, headers, routes},
    http::Client,
    identity::ApiUrl,
    ids::{IdempotencyKey, SendId},
    ipc::{
        cli::warnings,
        ui::{TtsBegin, TtsChunk, TtsEnd, TtsError},
    },
    runtime::{DELIVERY_MARGIN, RefreshPolicy, force_refresh},
    schema::limits,
};
use tokio::{io::AsyncWriteExt as _, task::AbortHandle};

use crate::{
    deepgram::{
        Deepgram, DgError, DgKind, ListenParams, RetryBudget, Transcript, classify_proxy_error,
        is_linear16, listen_query, parse_transcript,
    },
    net::HomeRef,
    paths::sha256_hex,
    state::{ActorKey, Shared, SharedRef},
    telemetry::Record,
    voice::estimated_frames,
};

/// TTS cache size cap (§1.7).
pub const TTS_CACHE_MAX_BYTES: u64 = 200 * 1024 * 1024;
/// TTS cache age cap (§1.7).
pub const TTS_CACHE_MAX_AGE: Duration = Duration::from_hours(30 * 24);
/// A JWT is reused until this long before it expires.
const TOKEN_SAFETY: Duration = Duration::from_secs(5);
/// Longest TTS exchange through the speech proxy, streaming included.
const PROXY_SPEAK_TIMEOUT: Duration = Duration::from_secs(180);

/// Whether a proxy token's `base_url` is exactly `<api>/api/v1/speech` of
/// the home's backend (same scheme, host and port): peekd sends a Silicon's
/// session nowhere else.
///
/// # Errors
/// `speech_unavailable` naming both URLs.
pub fn check_proxy_base(base_url: &str, api: &ApiUrl) -> Result<()> {
    let refuse = || {
        Error::new(
            ErrorCode::SpeechUnavailable,
            format!(
                "peek-server's speech proxy URL `{base_url}` is not {}{} (this Silicon's backend); peekd never sends a session elsewhere",
                api.as_str(),
                routes::SPEECH_BASE
            ),
        )
        .with_hint("the backend's PEEK_PUBLIC_ORIGIN must be the origin clients use (--api / PEEK_API_URL)")
    };
    let (Ok(base), Ok(origin)) = (url::Url::parse(base_url), url::Url::parse(api.as_str())) else {
        return Err(refuse());
    };
    let same_origin = base.scheme() == origin.scheme()
        && base.host_str() == origin.host_str()
        && base.port_or_known_default() == origin.port_or_known_default();
    let path_ok = base.path().trim_end_matches('/') == routes::SPEECH_BASE
        && base.query().is_none()
        && base.username().is_empty()
        && base.password().is_none();
    if same_origin && path_ok {
        Ok(())
    } else {
        Err(refuse())
    }
}

/// A session problem as a speech failure: retryable ones retry within the
/// budget, the rest end the attempt.
fn session_failure(e: Error) -> DgError {
    DgError {
        kind: if e.retryable() {
            DgKind::Retry
        } else {
            DgKind::Fatal
        },
        error: e,
        retry_after: None,
    }
}

/// The TTS cache.
#[derive(Debug)]
pub struct TtsCache {
    dir: PathBuf,
    max_bytes: u64,
    max_age: Duration,
    lock: Mutex<()>,
}

impl TtsCache {
    /// The cache in `dir` with the §1.7 limits.
    #[must_use]
    pub fn new(dir: PathBuf) -> Self {
        Self::with_limits(dir, TTS_CACHE_MAX_BYTES, TTS_CACHE_MAX_AGE)
    }

    /// The cache with explicit limits.
    #[must_use]
    pub fn with_limits(dir: PathBuf, max_bytes: u64, max_age: Duration) -> Self {
        Self {
            dir,
            max_bytes,
            max_age,
            lock: Mutex::new(()),
        }
    }

    /// `sha256(model|params|text)`.
    #[must_use]
    pub fn key(model: &str, text: &str) -> String {
        sha256_hex(
            format!("{model}|encoding=linear16&container=none&sample_rate=24000|{text}").as_bytes(),
        )
    }

    fn path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.pcm"))
    }

    /// The cached PCM for `key`, refreshed as most recently used.
    #[must_use]
    pub fn lookup(&self, key: &str) -> Option<PathBuf> {
        let _g = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        let p = self.path(key);
        let meta = std::fs::metadata(&p).ok()?;
        let age = meta
            .modified()
            .ok()
            .and_then(|m| SystemTime::now().duration_since(m).ok())
            .unwrap_or_default();
        if age > self.max_age || meta.len() == 0 {
            let _ = std::fs::remove_file(&p);
            return None;
        }
        if let Ok(f) = std::fs::File::options().write(true).open(&p) {
            let _ = f.set_modified(SystemTime::now());
        }
        Some(p)
    }

    /// A fresh temp path in the cache directory.
    #[must_use]
    pub fn temp_path(&self) -> PathBuf {
        self.dir
            .join(format!(".tmp-{}.pcm", uuid::Uuid::now_v7().simple()))
    }

    /// Moves a completed temp file into the cache and prunes.
    pub fn commit(&self, key: &str, temp: &Path) {
        {
            let _g = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
            if let Err(e) = std::fs::rename(temp, self.path(key)) {
                tracing::warn!(error = %e, "a TTS cache entry could not be stored");
                let _ = std::fs::remove_file(temp);
                return;
            }
        }
        self.prune();
    }

    /// Deletes entries older than the age cap, then the least recently used
    /// ones until the total fits the size cap.
    pub fn prune(&self) {
        let _g = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        let Ok(rd) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let now = SystemTime::now();
        let mut entries: Vec<(PathBuf, SystemTime, u64)> = Vec::new();
        for e in rd.flatten() {
            let p = e.path();
            let Ok(meta) = e.metadata() else { continue };
            let mtime = meta.modified().unwrap_or(now);
            let name = e.file_name().to_string_lossy().into_owned();
            let stale_temp = name.starts_with(".tmp-")
                && now.duration_since(mtime).unwrap_or_default() > Duration::from_secs(3600);
            if stale_temp || now.duration_since(mtime).unwrap_or_default() > self.max_age {
                let _ = std::fs::remove_file(&p);
                continue;
            }
            if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pcm"))
                && !name.starts_with(".tmp-")
            {
                entries.push((p, mtime, meta.len()));
            }
        }
        let mut total: u64 = entries.iter().map(|e| e.2).sum();
        entries.sort_by_key(|e| e.1);
        for (p, _, len) in entries {
            if total <= self.max_bytes {
                break;
            }
            if std::fs::remove_file(&p).is_ok() {
                total = total.saturating_sub(len);
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct TokenKey {
    api: String,
    context: String,
    org: String,
    actor: String,
    purpose: SpeechPurpose,
}

#[derive(Clone, Debug)]
struct CachedToken {
    token: SpeechToken,
    valid_until: Instant,
}

/// Deepgram access, the JWT cache, TTS tasks and the TTS cache.
#[derive(Debug)]
pub struct Speech {
    /// The Deepgram client.
    pub deepgram: Deepgram,
    tokens: Mutex<HashMap<TokenKey, CachedToken>>,
    tasks: Mutex<HashMap<SendId, AbortHandle>>,
    /// The TTS cache.
    pub cache: TtsCache,
}

impl Speech {
    /// A speech engine caching TTS audio in `cache_dir`.
    ///
    /// # Errors
    /// As [`Deepgram::new`].
    pub fn new(cache_dir: PathBuf) -> Result<Self> {
        Ok(Self {
            deepgram: Deepgram::new()?,
            tokens: Mutex::new(HashMap::new()),
            tasks: Mutex::new(HashMap::new()),
            cache: TtsCache::new(cache_dir),
        })
    }

    /// Stops the TTS stream of a send ("peekd drops the HTTP body").
    pub fn cancel(&self, send_id: &SendId) {
        if let Some(h) = self
            .tasks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(send_id)
        {
            h.abort();
        }
    }

    /// Stops every TTS stream (shutdown).
    pub fn cancel_all(&self) {
        for (_, h) in self
            .tasks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain()
        {
            h.abort();
        }
    }

    fn forget_token(&self, key: &TokenKey) {
        self.tokens
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(key);
    }
}

fn token_key(home: &HomeRef, key: &ActorKey, purpose: SpeechPurpose) -> TokenKey {
    TokenKey {
        api: home.api_url.as_str().to_owned(),
        context: key.context_str(),
        org: key.org.as_str().to_owned(),
        actor: key.actor.as_str().to_owned(),
        purpose,
    }
}

/// What to speak.
#[derive(Clone, Debug)]
pub struct TtsJob {
    /// The send.
    pub send_id: SendId,
    /// Its Silicon.
    pub key: ActorKey,
    /// Its home.
    pub home: HomeRef,
    /// The Aura-2 voice.
    pub model: String,
    /// The text.
    pub text: String,
}

impl Shared {
    /// A Deepgram JWT for `purpose` from cache, else `fresh_session` →
    /// `POST /api/v1/speech/token` (§5.2). `force` drops the cached one.
    ///
    /// # Errors
    /// `speech_unavailable`, session errors, transport errors.
    pub async fn speech_token(
        &self,
        home: &HomeRef,
        key: &ActorKey,
        purpose: SpeechPurpose,
        force: bool,
    ) -> Result<SpeechToken> {
        let tk = token_key(home, key, purpose);
        if force {
            self.speech.forget_token(&tk);
        } else if let Some(c) = self
            .speech
            .tokens
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&tk)
            .filter(|c| c.valid_until > Instant::now())
        {
            return Ok(c.token.clone());
        }
        let client = self.speech_session(home, key, false).await?;
        let token = match client
            .speech_token(purpose, &IdempotencyKey::generate())
            .await
        {
            // The backend refused the access token: refresh once and retry.
            Err(e) if e.status() == Some(401) && *e.code() != ErrorCode::SessionRejected => {
                self.speech_session(home, key, true)
                    .await?
                    .speech_token(purpose, &IdempotencyKey::generate())
                    .await?
            }
            // The testing environment moved on: save its new generation, retry once.
            Err(e)
                if *e.code() == ErrorCode::TestingGenerationChanged
                    && home.context.is_testing() =>
            {
                self.net.refresh_generation(home).await?;
                self.speech_session(home, key, false)
                    .await?
                    .speech_token(purpose, &IdempotencyKey::generate())
                    .await?
            }
            other => other?,
        };
        let ttl = Duration::from_secs(token.expires_in.min(3600)).saturating_sub(TOKEN_SAFETY);
        self.speech
            .tokens
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                tk,
                CachedToken {
                    token: token.clone(),
                    valid_until: Instant::now() + ttl,
                },
            );
        Ok(token)
    }

    /// A client carrying the Silicon's own session for `home` (refreshed
    /// first when `force`, after the backend refused the access token).
    ///
    /// # Errors
    /// Session errors; `authority_required` when the home now holds another
    /// Silicon.
    pub async fn speech_session(
        &self,
        home: &HomeRef,
        key: &ActorKey,
        force: bool,
    ) -> Result<Client> {
        let policy = RefreshPolicy::with_delays(self.cfg.timings.refresh_retry.clone());
        let (client, slot, store) = self.net.session(home, DELIVERY_MARGIN, &policy).await?;
        let (client, slot) = if force {
            let (_, base) = self.net.client(home)?;
            let fresh = force_refresh(
                &store,
                &base,
                home.context,
                &slot.access_token,
                &RefreshPolicy::with_delays(Vec::new()),
            )
            .await?;
            (
                base.with_session(fresh.access_token.clone(), fresh.org_id.clone()),
                fresh,
            )
        } else {
            (client, slot)
        };
        if slot.actor.public_id != key.actor || slot.org_id != key.org {
            return Err(Error::new(
                ErrorCode::AuthorityRequired,
                format!(
                    "{} now holds a session for {} in {}, not {}; peek never speaks with another Silicon's authority",
                    home.home_path, slot.actor.public_id, slot.org_id, key.actor
                ),
            )
            .with_hint(silicon_peek_client::runtime::session::RELOGIN_HINT));
        }
        Ok(client)
    }

    /// Starts `/v1/speak` in the token's mode; returns the response once its
    /// status and content type say the body is raw 24 kHz s16le audio.
    async fn speak_via(
        &self,
        job: &TtsJob,
        token: &SpeechToken,
        first_byte: Duration,
        fresh_session: bool,
    ) -> std::result::Result<(reqwest::Response, Option<String>), DgError> {
        if token.mode == SpeechMode::Direct {
            return self
                .speech
                .deepgram
                .speak(token, &job.model, &job.text, first_byte)
                .await;
        }
        check_proxy_base(&token.base_url, &job.home.api_url).map_err(|e| DgError {
            kind: DgKind::Fatal,
            error: e,
            retry_after: None,
        })?;
        // The session (possibly a refresh) and the proxied call share one
        // first-byte deadline.
        let deadline = tokio::time::Instant::now() + first_byte;
        let no_first_byte = || DgError {
            kind: DgKind::Retry,
            error: Error::new(
                ErrorCode::SpeechUnavailable,
                format!(
                    "peek-server's speech proxy produced no audio within {} ms",
                    first_byte.as_millis()
                ),
            )
            .with_retryable(true),
            retry_after: None,
        };
        let client = match tokio::time::timeout_at(
            deadline,
            self.speech_session(&job.home, &job.key, fresh_session),
        )
        .await
        {
            Ok(r) => r.map_err(session_failure)?,
            Err(_) => return Err(no_first_byte()),
        };
        let request = SpeechSpeakRequest {
            text: job.text.clone(),
            model: job.model.clone(),
            sample_rate: Some(24_000),
        };
        let key = IdempotencyKey::generate();
        let call = client.speech_speak(&request, &key, PROXY_SPEAK_TIMEOUT);
        let resp = match tokio::time::timeout_at(deadline, call).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => return Err(classify_proxy_error(e)),
            Err(_) => return Err(no_first_byte()),
        };
        let header = |name: &str| {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        let ctype = header(reqwest::header::CONTENT_TYPE.as_str()).unwrap_or_default();
        if !is_linear16(&ctype) {
            return Err(DgError {
                kind: DgKind::Fatal,
                error: Error::new(
                    ErrorCode::SpeechUnavailable,
                    format!("the speech proxy returned `{ctype}` instead of raw linear16 audio"),
                ),
                retry_after: None,
            });
        }
        let request_id = header(headers::DG_REQUEST_ID);
        Ok((resp, request_id))
    }

    /// `/v1/listen` in the token's mode.
    #[allow(clippy::too_many_arguments)] // one call site (stt.rs); the parts of one attempt
    pub(crate) async fn listen_via(
        &self,
        home: &HomeRef,
        key: &ActorKey,
        token: &SpeechToken,
        params: &ListenParams,
        wav: Vec<u8>,
        timeout: Duration,
        fresh_session: bool,
    ) -> std::result::Result<Transcript, DgError> {
        if token.mode == SpeechMode::Direct {
            return self
                .speech
                .deepgram
                .listen(token, params, wav, timeout)
                .await;
        }
        check_proxy_base(&token.base_url, &home.api_url).map_err(|e| DgError {
            kind: DgKind::Fatal,
            error: e,
            retry_after: None,
        })?;
        let client = self
            .speech_session(home, key, fresh_session)
            .await
            .map_err(session_failure)?;
        let (body, request_id) = client
            .speech_listen(
                &listen_query(params),
                wav,
                "audio/wav",
                &IdempotencyKey::generate(),
                timeout,
            )
            .await
            .map_err(classify_proxy_error)?;
        parse_transcript(&body, request_id)
    }

    /// Starts streaming speech for a bubble that just reached the UI.
    pub fn start_tts(self: &SharedRef, job: TtsJob) {
        let this = Arc::clone(self);
        let send_id = job.send_id.clone();
        let handle = tokio::spawn(async move {
            let id = job.send_id.clone();
            this.run_tts(job).await;
            this.speech
                .tasks
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&id);
        });
        let previous = self
            .speech
            .tasks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(send_id, handle.abort_handle());
        if let Some(p) = previous {
            p.abort();
        }
    }

    async fn run_tts(self: &SharedRef, job: TtsJob) {
        let started = Instant::now();
        let cache_key = TtsCache::key(&job.model, &job.text);
        let chars = job.text.chars().count();
        let est = estimated_frames(chars);
        let mut record = Record::new("tts.request", "ok")
            .with("tts_model", job.model.clone())
            .with("speak_chars", chars);
        record.actor = Some((job.key.org.clone(), job.key.actor.clone()));
        record.testing = job.key.context.is_testing();
        let outcome = if let Some(path) = self.speech.cache.lookup(&cache_key) {
            record = record.with("status", "cached");
            self.stream_file(&job.send_id, &path, est)
                .await
                .map_err(|e| (e, false))
        } else {
            record = record.with("status", "streamed");
            self.synthesize(&job, est, &cache_key, started, &mut record)
                .await
        };
        if let Err((e, audio_started)) = outcome {
            record.outcome = "error";
            record.error_code = Some(e.code().to_string());
            if !audio_started {
                self.tts_failed(&job.send_id, &e).await;
            }
        }
        record.duration_ms = Some(millis(started.elapsed()));
        self.record(record);
    }

    /// Mint → speak → stream, retrying only before the first audio byte
    /// (§1.9.3 step 7). The error carries whether audio had started.
    async fn synthesize(
        &self,
        job: &TtsJob,
        est: u64,
        cache_key: &str,
        started: Instant,
        record: &mut Record,
    ) -> std::result::Result<(), (Error, bool)> {
        let mut budget = RetryBudget::new(
            started + self.cfg.timings.tts_first_audio_budget,
            &self.cfg.timings.tts_retry,
        );
        let mut force_token = false;
        let mut fresh_session = false;
        let no_audio = || {
            Error::new(
                ErrorCode::SpeechUnavailable,
                format!(
                    "no audio within the {} s budget",
                    self.cfg.timings.tts_first_audio_budget.as_secs()
                ),
            )
            .with_retryable(true)
        };
        loop {
            // Minting (and the session refresh behind it) spends the same
            // first-audio budget: a slow backend fails fast into tts.error
            // and the text pill instead of after its 30 s request timeout.
            let minted = tokio::time::timeout(
                budget.remaining(),
                self.speech_token(&job.home, &job.key, SpeechPurpose::Tts, force_token),
            )
            .await;
            let Ok(minted) = minted else {
                return Err((no_audio(), false));
            };
            let token = match minted {
                Ok(t) => t,
                Err(e) => match budget.next(if e.retryable() {
                    DgKind::Retry
                } else {
                    DgKind::Fatal
                }) {
                    Some(d) => {
                        tokio::time::sleep(jitter(d)).await;
                        continue;
                    }
                    None => return Err((e, false)),
                },
            };
            let remaining = budget.remaining();
            if remaining.is_zero() {
                return Err((no_audio(), false));
            }
            match self.speak_via(job, &token, remaining, fresh_session).await {
                Ok((resp, request_id)) => {
                    *record = std::mem::take(record).with(
                        "method",
                        match token.mode {
                            SpeechMode::Direct => "direct",
                            SpeechMode::Proxy => "proxy",
                        },
                    );
                    if let Some(rid) = request_id {
                        *record = std::mem::take(record).with("dg_request_id", rid);
                    }
                    match self
                        .stream_response(job, resp, est, cache_key, started, record)
                        .await
                    {
                        StreamOutcome::Done => return Ok(()),
                        StreamOutcome::FailedAfterAudio(e) => return Err((e, true)),
                        StreamOutcome::FailedBeforeAudio(e) => match budget.next(DgKind::Retry) {
                            Some(d) => tokio::time::sleep(jitter(d)).await,
                            None => return Err((e, false)),
                        },
                    }
                }
                Err(DgError {
                    kind: DgKind::Reauth,
                    error,
                    ..
                }) => {
                    if !budget.reauth() {
                        return Err((error, false));
                    }
                    // Direct: the JWT was refused, mint a new one. Proxy:
                    // the backend refused the session, refresh it.
                    match token.mode {
                        SpeechMode::Direct => force_token = true,
                        SpeechMode::Proxy => fresh_session = true,
                    }
                }
                Err(DgError {
                    kind,
                    error,
                    retry_after,
                }) => match budget.next(kind) {
                    Some(d) => tokio::time::sleep(retry_after.unwrap_or_else(|| jitter(d))).await,
                    None => return Err((error, false)),
                },
            }
        }
    }

    async fn stream_response(
        &self,
        job: &TtsJob,
        mut resp: reqwest::Response,
        est: u64,
        cache_key: &str,
        started: Instant,
        record: &mut Record,
    ) -> StreamOutcome {
        let temp = self.speech.cache.temp_path();
        let mut file = open_private(&temp).await;
        let mut seq = 0u64;
        let mut total = 0u64;
        let mut carry: Option<u8> = None;
        let idle = self.cfg.timings.tts_idle;
        loop {
            let chunk = match tokio::time::timeout(idle, resp.chunk()).await {
                Ok(Ok(Some(c))) => c,
                Ok(Ok(None)) => break,
                Ok(Err(e)) => {
                    let err = Error::new(
                        ErrorCode::SpeechUnavailable,
                        format!("the Deepgram audio stream broke: {e}"),
                    )
                    .with_retryable(true);
                    return self.abandon(job, &temp, seq, total, err).await;
                }
                Err(_) => {
                    let err = Error::new(
                        ErrorCode::SpeechUnavailable,
                        format!("the Deepgram audio stream stalled for {} s", idle.as_secs()),
                    )
                    .with_retryable(true);
                    return self.abandon(job, &temp, seq, total, err).await;
                }
            };
            let mut bytes: Vec<u8> = Vec::with_capacity(chunk.len() + 1);
            if let Some(b) = carry.take() {
                bytes.push(b);
            }
            bytes.extend_from_slice(&chunk);
            if bytes.len() % 2 == 1 {
                carry = bytes.pop();
            }
            if bytes.is_empty() {
                continue;
            }
            if seq == 0 {
                *record = std::mem::take(record).with("tts_ttfb_ms", millis(started.elapsed()));
                self.ui
                    .event(&TtsBegin::aura(job.send_id.clone(), est), Vec::new());
            }
            if let Some(f) = file.as_mut()
                && f.write_all(&bytes).await.is_err()
            {
                file = None;
                let _ = tokio::fs::remove_file(&temp).await;
            }
            for piece in bytes.chunks(limits::TTS_CHUNK_MAX_BYTES) {
                self.ui.event(
                    &TtsChunk {
                        send_id: job.send_id.clone(),
                        seq,
                    },
                    vec![piece.to_vec()],
                );
                seq += 1;
                total += u64::try_from(piece.len()).unwrap_or(0);
            }
        }
        if seq == 0 {
            let _ = tokio::fs::remove_file(&temp).await;
            return StreamOutcome::FailedBeforeAudio(
                Error::new(
                    ErrorCode::SpeechUnavailable,
                    "Deepgram returned an empty audio stream",
                )
                .with_retryable(true),
            );
        }
        self.ui.event(
            &TtsEnd {
                send_id: job.send_id.clone(),
                total_frames: total / 2,
            },
            Vec::new(),
        );
        if let Some(mut f) = file
            && f.flush().await.is_ok()
            && f.sync_all().await.is_ok()
        {
            drop(f);
            let cache = &self.speech.cache;
            cache.commit(cache_key, &temp);
        } else {
            let _ = tokio::fs::remove_file(&temp).await;
        }
        StreamOutcome::Done
    }

    /// A stream that broke: before any audio it may be retried; after audio
    /// the stream ends early (never repeat speech).
    async fn abandon(
        &self,
        job: &TtsJob,
        temp: &Path,
        seq: u64,
        total: u64,
        err: Error,
    ) -> StreamOutcome {
        let _ = tokio::fs::remove_file(temp).await;
        if seq == 0 {
            return StreamOutcome::FailedBeforeAudio(err);
        }
        tracing::warn!(send = %job.send_id, error = %err, "TTS stream ended early after audio started");
        self.ui.event(
            &TtsEnd {
                send_id: job.send_id.clone(),
                total_frames: total / 2,
            },
            Vec::new(),
        );
        StreamOutcome::FailedAfterAudio(err)
    }

    async fn stream_file(&self, send_id: &SendId, path: &Path, est: u64) -> Result<()> {
        let bytes = tokio::fs::read(path).await.map_err(|e| {
            Error::internal(format!(
                "reading the TTS cache entry {} failed: {e}",
                path.display()
            ))
        })?;
        self.ui
            .event(&TtsBegin::aura(send_id.clone(), est), Vec::new());
        let usable = bytes.len() - bytes.len() % 2;
        for (seq, piece) in (0u64..).zip(bytes[..usable].chunks(limits::TTS_CHUNK_MAX_BYTES)) {
            self.ui.event(
                &TtsChunk {
                    send_id: send_id.clone(),
                    seq,
                },
                vec![piece.to_vec()],
            );
        }
        self.ui.event(
            &TtsEnd {
                send_id: send_id.clone(),
                total_frames: u64::try_from(usable / 2).unwrap_or(0),
            },
            Vec::new(),
        );
        Ok(())
    }

    /// TTS failed before any audio: tell the UI (it shows the text as a pill
    /// when there is no `--show`) and note `speech_failed` in the history.
    async fn tts_failed(&self, send_id: &SendId, e: &Error) {
        tracing::warn!(send = %send_id, error = %e, "TTS failed before any audio");
        self.ui.event(
            &TtsError {
                send_id: send_id.clone(),
                error: e.to_object(),
            },
            Vec::new(),
        );
        let code = if *e.code() == ErrorCode::SpeechUnavailable && !e.retryable() {
            warnings::SPEECH_UNAVAILABLE
        } else {
            warnings::SPEECH_FAILED
        };
        let warning = json!({"code": code, "message": format!("speech failed before any audio played: {}", e.message())});
        let id = send_id.as_str().to_owned();
        let r = self
            .db
            .call(move |c| {
                crate::bubbles::set_speech_outcome(
                    c,
                    &id,
                    crate::bubbles::StoredSpeechStatus::Failed,
                )?;
                crate::bubbles::append_send_warning(c, &id, warning)
            })
            .await;
        if let Err(e) = r {
            tracing::warn!(error = %e, "could not record a speech warning");
        }
    }
}

enum StreamOutcome {
    Done,
    FailedBeforeAudio(Error),
    FailedAfterAudio(Error),
}

async fn open_private(path: &Path) -> Option<tokio::fs::File> {
    tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .await
        .ok()
}

/// Up to +25 % jitter.
#[must_use]
pub fn jitter(d: Duration) -> Duration {
    let r = uuid::Uuid::new_v4().as_u128() % 250;
    d + d.mul_f64(f64::from(u32::try_from(r).unwrap_or(0)) / 1000.0)
}

/// Whole milliseconds.
#[must_use]
pub fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_lru_and_age() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        let cache = TtsCache::with_limits(dir.path().to_path_buf(), 25, Duration::from_secs(3600));
        let k1 = TtsCache::key("aura-2-thalia-en", "one");
        let k2 = TtsCache::key("aura-2-thalia-en", "two");
        assert_ne!(k1, k2);
        assert_ne!(k1, TtsCache::key("aura-2-apollo-en", "one"));
        for (k, len) in [(&k1, 10usize), (&k2, 10)] {
            let t = cache.temp_path();
            std::fs::write(&t, vec![0u8; len])?;
            cache.commit(k, &t);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(cache.lookup(&k1).is_some(), "touching k1 makes k2 the LRU");
        std::thread::sleep(Duration::from_millis(20));
        let k3 = TtsCache::key("aura-2-thalia-en", "three");
        let t = cache.temp_path();
        std::fs::write(&t, vec![0u8; 10])?;
        cache.commit(&k3, &t);
        assert!(cache.lookup(&k1).is_some());
        assert!(cache.lookup(&k2).is_none(), "LRU evicted");
        assert!(cache.lookup(&k3).is_some());
        let aged = TtsCache::with_limits(dir.path().to_path_buf(), 1000, Duration::ZERO);
        assert!(
            aged.lookup(&k1).is_none(),
            "entries past the age cap are gone"
        );
        Ok(())
    }

    #[test]
    fn proxy_base_must_be_the_homes_backend() -> Result<()> {
        let api = ApiUrl::parse("http://127.0.0.1:8080")?;
        check_proxy_base("http://127.0.0.1:8080/api/v1/speech", &api)?;
        check_proxy_base("http://127.0.0.1:8080/api/v1/speech/", &api)?;
        for bad in [
            "http://127.0.0.1:9999/api/v1/speech",
            "http://localhost:8080/api/v1/speech",
            "https://127.0.0.1:8080/api/v1/speech",
            "http://127.0.0.1:8080/api/v1/other",
            "http://127.0.0.1:8080/api/v1/speech?x=1",
            "not a url",
        ] {
            assert!(check_proxy_base(bad, &api).is_err(), "{bad}");
        }
        let prod = ApiUrl::production();
        check_proxy_base(
            "https://backend.peek.teamofsilicons.com/api/v1/speech",
            &prod,
        )?;
        check_proxy_base(
            "https://backend.peek.teamofsilicons.com:443/api/v1/speech",
            &prod,
        )?;
        Ok(())
    }

    #[test]
    fn jitter_bounds() {
        let d = Duration::from_millis(1000);
        for _ in 0..50 {
            let j = jitter(d);
            assert!(j >= d && j < Duration::from_millis(1250));
        }
    }
}
