//! Stateless HTTP client for peek-server (BLUEPRINT §5.2).
//!
//! [`Client`] stores no session and never refreshes behind the caller's back.
//! It attaches, per request, exactly the headers of the CLI/peekd → peek-server
//! hop (§2.9): `Authorization` + `X-Org-ID` on bearer routes,
//! `Idempotency-Key` on every POST, `X-Testing-Environment-Key` on every route
//! in a testing context, `X-Testing-Environment-Generation` on every mutation
//! except login and refresh, and `X-Peek-Telemetry: on|off`.
//!
//! Failures decode into [`Error`]: the server's `{"error":{…}}` object when
//! there is one, otherwise a precise synthesized error. Transport failures are
//! `backend_unavailable`, retryable, with [`Origin::Transport`].

use std::time::Duration;

use reqwest::{Method, RequestBuilder, StatusCode, header::HeaderMap};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

use crate::{
    Secret,
    api::{
        ByoDeepgramRequest, ByoStatus, DeliveryResponse, Drawing, DrawingStored, Health,
        IamDiscovery, LoginRequest, LogoutRequest, Me, Ready, RefreshRequest, ReportRequest,
        ReportResponse, SessionResponse, SpeechPurpose, SpeechSpeakRequest, SpeechToken,
        SpeechTokenRequest, TelemetryBatch, TingRecipient, headers, routes,
    },
    error::{Error, ErrorCode, ErrorObject, Origin, Result},
    identity::{ApiUrl, OrgId, TestingSecret},
    ids::{EventId, IdempotencyKey},
    schema::{check_drawing_bytes, limits},
};

/// Largest response body accepted.
pub const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

/// Default whole-request timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Default connect timeout.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Installs rustls' `ring` provider as the process default, if none is set.
/// Every binary calls this first in `main`; the client calls it too, so tests
/// and library users never depend on that order.
pub fn ensure_crypto_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}

/// Configures a [`Client`].
#[derive(Debug)]
pub struct ClientBuilder {
    api: ApiUrl,
    timeout: Duration,
    connect_timeout: Duration,
    user_agent: String,
    http: Option<reqwest::Client>,
}

impl ClientBuilder {
    /// Whole-request timeout (default 30 s).
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Connect timeout (default 5 s).
    #[must_use]
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Prefixes the user agent with the calling component, e.g. `peek-cli/0.1.0`.
    #[must_use]
    pub fn component(mut self, component: &str) -> Self {
        self.user_agent = format!("{component} {}", self.user_agent);
        self
    }

    /// Reuses an existing connection pool (peekd shares one across homes).
    /// Its timeouts and redirect policy are the caller's.
    #[must_use]
    pub fn http_client(mut self, http: reqwest::Client) -> Self {
        self.http = Some(http);
        self
    }

    /// Builds the client.
    ///
    /// # Errors
    /// `internal_error` if the TLS stack cannot initialize.
    pub fn build(self) -> Result<Client> {
        ensure_crypto_provider();
        let http = match self.http {
            Some(http) => http,
            None => reqwest::Client::builder()
                .timeout(self.timeout)
                .connect_timeout(self.connect_timeout)
                .redirect(reqwest::redirect::Policy::none())
                .user_agent(self.user_agent)
                .build()
                .map_err(|e| {
                    Error::internal(format!("could not initialize the HTTP client: {e}"))
                        .with_source(e)
                })?,
        };
        Ok(Client {
            http,
            api: self.api,
            timeout: self.timeout,
            session: None,
            testing: None,
            telemetry: true,
            trace_id: None,
        })
    }
}

#[derive(Clone, Debug)]
struct Testing {
    secret: TestingSecret,
    generation: Option<u64>,
}

/// A stateless peek-server client. Cheap to clone; `with_*` return copies.
#[derive(Clone, Debug)]
pub struct Client {
    http: reqwest::Client,
    api: ApiUrl,
    timeout: Duration,
    session: Option<(Secret, OrgId)>,
    testing: Option<Testing>,
    telemetry: bool,
    trace_id: Option<String>,
}

impl Client {
    /// A client with default settings.
    ///
    /// # Errors
    /// As [`ClientBuilder::build`].
    pub fn new(api: &ApiUrl) -> Result<Self> {
        Self::builder(api).build()
    }

    /// A builder.
    #[must_use]
    pub fn builder(api: &ApiUrl) -> ClientBuilder {
        ClientBuilder {
            api: api.clone(),
            timeout: DEFAULT_TIMEOUT,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            user_agent: concat!("silicon-peek-client/", env!("CARGO_PKG_VERSION")).to_owned(),
            http: None,
        }
    }

    /// The API origin.
    #[must_use]
    pub fn api_url(&self) -> &ApiUrl {
        &self.api
    }

    /// Attaches a Silicon's access token and org (`Authorization` + `X-Org-ID`).
    #[must_use]
    pub fn with_session(&self, access_token: Secret, org: OrgId) -> Self {
        let mut next = self.clone();
        next.session = Some((access_token, org));
        next
    }

    /// Drops the session.
    #[must_use]
    pub fn without_session(&self) -> Self {
        let mut next = self.clone();
        next.session = None;
        next
    }

    /// Selects a testing environment by its peek test app secret. The
    /// generation is sent on mutations once known (from [`Client::discover`]).
    #[must_use]
    pub fn with_testing(&self, secret: TestingSecret, generation: Option<u64>) -> Self {
        let mut next = self.clone();
        next.testing = Some(Testing { secret, generation });
        next
    }

    /// Whether a testing environment is selected.
    #[must_use]
    pub fn is_testing(&self) -> bool {
        self.testing.is_some()
    }

    /// Sets `X-Peek-Telemetry` (default `on`); `off` also suppresses
    /// [`Client::telemetry`].
    #[must_use]
    pub fn with_telemetry(&self, enabled: bool) -> Self {
        let mut next = self.clone();
        next.telemetry = enabled;
        next
    }

    /// Carries a trace ID (`X-Peek-Trace-Id`) on every request.
    #[must_use]
    pub fn with_trace_id(&self, trace_id: impl Into<String>) -> Self {
        let mut next = self.clone();
        next.trace_id = Some(trace_id.into());
        next
    }

    fn request(&self, method: Method, path: &str, generation: bool) -> RequestBuilder {
        let mut r = self
            .http
            .request(method, self.api.join(path))
            .header(headers::CLIENT_VERSION, crate::VERSION)
            .header(
                headers::TELEMETRY,
                if self.telemetry { "on" } else { "off" },
            )
            .header(reqwest::header::ACCEPT, "application/json");
        if let Some(t) = &self.trace_id {
            r = r.header(headers::TRACE_ID, t);
        }
        if let Some(t) = &self.testing
            && path.starts_with("/api/")
        {
            r = r.header(headers::TESTING_KEY, t.secret.secret().expose());
            if generation && let Some(g) = t.generation {
                r = r.header(headers::TESTING_GENERATION, g.to_string());
            }
        }
        r
    }

    fn bearer(&self, r: RequestBuilder) -> Result<RequestBuilder> {
        let (token, org) = self.session.as_ref().ok_or_else(|| {
            Error::new(
                ErrorCode::NotLoggedIn,
                "this peek-server route needs a session, and none is attached",
            )
            .with_hint("log in first: peek login '<SLT>'")
        })?;
        Ok(r.bearer_auth(token.expose())
            .header(headers::ORG_ID, org.as_str()))
    }

    fn post<B: Serialize + ?Sized>(
        &self,
        path: &str,
        key: &IdempotencyKey,
        body: &B,
        generation: bool,
    ) -> RequestBuilder {
        self.request(Method::POST, path, generation)
            .header(headers::IDEMPOTENCY_KEY, key.as_str())
            .json(body)
    }

    /// `GET /healthz`.
    ///
    /// # Errors
    /// Transport or server errors.
    pub async fn healthz(&self) -> Result<Health> {
        self.json(
            routes::HEALTHZ,
            self.request(Method::GET, routes::HEALTHZ, false),
        )
        .await
    }

    /// `GET /readyz`. A 503 with a readiness body is returned as `Ok` with
    /// `status != "ready"`.
    ///
    /// # Errors
    /// Transport errors, or a 503 without a readiness body.
    pub async fn readyz(&self) -> Result<Ready> {
        let (status, _, bytes) = self
            .send(
                routes::READYZ,
                self.request(Method::GET, routes::READYZ, false),
            )
            .await?;
        if status == StatusCode::SERVICE_UNAVAILABLE
            && let Ok(ready) = serde_json::from_slice::<Ready>(&bytes)
        {
            return Ok(ready);
        }
        self.check(routes::READYZ, status, &HeaderMap::new(), &bytes)?;
        decode(routes::READYZ, &bytes)
    }

    /// `GET /api/v1/iam`; with a testing secret it reports the environment and
    /// its generation.
    ///
    /// # Errors
    /// Transport or server errors (`testing_secret_invalid` for a bad secret).
    pub async fn discover(&self) -> Result<IamDiscovery> {
        self.json(routes::IAM, self.request(Method::GET, routes::IAM, false))
            .await
    }

    /// `POST /api/v1/auth/login` with body exactly `{"slt":"…"}`. Retry a
    /// transport failure or 5xx with the same key and SLT (IAM replays).
    ///
    /// # Errors
    /// `slt_rejected`, `slt_is_public_id`, `private_application_organization_required`,
    /// `iam_misconfigured`, transport errors.
    pub async fn login(
        &self,
        slt: &Secret,
        org_hint: Option<&OrgId>,
        key: &IdempotencyKey,
    ) -> Result<SessionResponse> {
        let body = LoginRequest { slt: slt.clone() };
        let mut r = self.post(routes::LOGIN, key, &body, false);
        if let Some(org) = org_hint {
            r = r.header(headers::ORG_ID, org.as_str());
        }
        self.json(routes::LOGIN, r).await
    }

    /// `POST /api/v1/auth/refresh` with body exactly `{"refresh_token":"…"}`.
    ///
    /// # Errors
    /// `session_rejected` (terminal), `idempotency_in_progress`,
    /// `idempotency_response_expired`, `iam_misconfigured`, transport errors.
    pub async fn refresh(
        &self,
        refresh_token: &Secret,
        key: &IdempotencyKey,
    ) -> Result<SessionResponse> {
        let body = RefreshRequest {
            refresh_token: refresh_token.clone(),
        };
        self.json(
            routes::REFRESH,
            self.post(routes::REFRESH, key, &body, false),
        )
        .await
    }

    /// `POST /api/v1/auth/logout` `{"token":"ort_…"}`. Only with
    /// `revoke_ting` (and a session attached) are `"revoke_ting":true` and the
    /// bearer sent, so the backend also revokes the Silicon's Ting grant; a
    /// plain logout never sends the bearer, so no backend revokes the grant
    /// that other homes of the same Silicon still use. Unknown tokens also
    /// succeed.
    ///
    /// # Errors
    /// Transport or server errors.
    pub async fn logout(
        &self,
        refresh_token: &Secret,
        key: &IdempotencyKey,
        revoke_ting: bool,
    ) -> Result<()> {
        let revoke_ting = revoke_ting && self.session.is_some();
        let body = LogoutRequest {
            token: refresh_token.clone(),
            revoke_ting,
        };
        let mut r = self.post(routes::LOGOUT, key, &body, true);
        if revoke_ting {
            r = self.bearer(r)?;
        }
        self.empty(routes::LOGOUT, r).await
    }

    /// `GET /api/v1/auth/me`: the live-introspected identity.
    ///
    /// # Errors
    /// `unauthenticated` (401), `reconsent_required`, transport errors.
    pub async fn me(&self) -> Result<Me> {
        let r = self.bearer(self.request(Method::GET, routes::ME, false))?;
        self.json(routes::ME, r).await
    }

    /// `POST /api/v1/ting/recipient` `{}`: (re)enrolls this Silicon as a Ting
    /// recipient. Never call this automatically from the delivery path.
    ///
    /// # Errors
    /// Server or transport errors.
    pub async fn ting_enroll(&self, key: &IdempotencyKey) -> Result<TingRecipient> {
        let r =
            self.bearer(self.post(routes::TING_RECIPIENT, key, &serde_json::json!({}), true))?;
        self.json(routes::TING_RECIPIENT, r).await
    }

    /// `POST /api/v1/deliveries` with the outbox row's exact bytes and key
    /// `peek-delivery-<event_id>`.
    ///
    /// # Errors
    /// `recipient_not_registered` (409), `reconsent_required`, `ting_unavailable`
    /// (with `retry_after`), `ting_type_missing`, `ting_key_conflict`, …
    pub async fn deliver(&self, event_id: &EventId, body: &[u8]) -> Result<DeliveryResponse> {
        let key = IdempotencyKey::delivery(event_id);
        let r = self
            .request(Method::POST, routes::DELIVERIES, true)
            .header(headers::IDEMPOTENCY_KEY, key.as_str())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body.to_vec());
        let r = self.bearer(r)?;
        self.json(routes::DELIVERIES, r).await
    }

    /// `POST /api/v1/speech/token`: a Deepgram JWT (≤ 60 s) in
    /// [`SpeechMode::Direct`](crate::api::SpeechMode), or the proxy verdict
    /// when peek-server's key cannot mint JWTs.
    ///
    /// # Errors
    /// `speech_unavailable` (with `details.reason`), server or transport errors.
    pub async fn speech_token(
        &self,
        purpose: SpeechPurpose,
        key: &IdempotencyKey,
    ) -> Result<SpeechToken> {
        let body = SpeechTokenRequest { purpose };
        let r = self.bearer(self.post(routes::SPEECH_TOKEN, key, &body, true))?;
        self.json(routes::SPEECH_TOKEN, r).await
    }

    /// `POST /api/v1/speech/speak` (speech proxy): peek-server calls Deepgram
    /// Aura-2 and streams its linear16 audio back unbuffered. Returns the
    /// response once the status is 200 (with `dg-request-id` /
    /// `dg-char-count` passed through); the caller drains the body. `timeout`
    /// bounds the whole exchange, body included.
    ///
    /// # Errors
    /// `speech_unavailable` (with `details.reason`), `unauthenticated`,
    /// `rate_limited`, server or transport errors.
    pub async fn speech_speak(
        &self,
        request: &SpeechSpeakRequest,
        key: &IdempotencyKey,
        timeout: Duration,
    ) -> Result<reqwest::Response> {
        let r = self
            .bearer(self.post(routes::SPEECH_SPEAK, key, request, true))?
            .timeout(timeout);
        let response = r
            .send()
            .await
            .map_err(|e| self.transport(routes::SPEECH_SPEAK, &e))?;
        if response.status().is_success() {
            return Ok(response);
        }
        let (status, headers, bytes) = self.drain(routes::SPEECH_SPEAK, response).await?;
        Err(decode_error(
            &self.api,
            routes::SPEECH_SPEAK,
            status,
            &headers,
            &bytes,
        ))
    }

    /// `POST /api/v1/speech/listen` (speech proxy): uploads recorded audio
    /// (≤ 4 MiB) with the allow-listed Deepgram query parameters
    /// ([`LISTEN_PARAMS`](crate::api::LISTEN_PARAMS)) and returns Deepgram's
    /// JSON result bytes and its `dg-request-id`.
    ///
    /// # Errors
    /// `speech_unavailable`, `invalid_input` for a refused parameter,
    /// `payload_too_large`, server or transport errors.
    pub async fn speech_listen(
        &self,
        query: &[(String, String)],
        audio: Vec<u8>,
        content_type: &str,
        key: &IdempotencyKey,
        timeout: Duration,
    ) -> Result<(Vec<u8>, Option<String>)> {
        let r = self
            .request(Method::POST, routes::SPEECH_LISTEN, true)
            .query(query)
            .header(headers::IDEMPOTENCY_KEY, key.as_str())
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .timeout(timeout)
            .body(audio);
        let r = self.bearer(r)?;
        let (status, response_headers, bytes) = self.send(routes::SPEECH_LISTEN, r).await?;
        self.check(routes::SPEECH_LISTEN, status, &response_headers, &bytes)?;
        let request_id = response_headers
            .get(headers::DG_REQUEST_ID)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        Ok((bytes, request_id))
    }

    /// `PUT /api/v1/drawings/current` with the raw script.
    ///
    /// # Errors
    /// `drawing_too_large` locally; server or transport errors.
    pub async fn put_drawing(&self, script: &[u8]) -> Result<DrawingStored> {
        check_drawing_bytes("drawing", script)?;
        let sha = hex::encode(Sha256::digest(script));
        let r = self
            .request(Method::PUT, routes::DRAWING, true)
            .header(reqwest::header::CONTENT_TYPE, "application/javascript")
            .header(headers::DRAWING_SHA256, sha)
            .body(script.to_vec());
        let r = self.bearer(r)?;
        self.json(routes::DRAWING, r).await
    }

    /// `GET /api/v1/drawings/current`; `None` when the backend has none.
    ///
    /// # Errors
    /// `unexpected_response` when the body does not match its `ETag`.
    pub async fn get_drawing(&self) -> Result<Option<Drawing>> {
        let r = self.bearer(self.request(Method::GET, routes::DRAWING, false))?;
        let (status, headers, bytes) = self.send(routes::DRAWING, r).await?;
        if status == StatusCode::NOT_FOUND {
            let e = self.check(routes::DRAWING, status, &headers, &bytes).err();
            match e {
                Some(e) if *e.code() == ErrorCode::DrawingNotFound => return Ok(None),
                Some(e) => return Err(e),
                None => return Ok(None),
            }
        }
        self.check(routes::DRAWING, status, &headers, &bytes)?;
        if bytes.len() > limits::DRAWING_MAX_BYTES {
            return Err(unexpected(routes::DRAWING, "the drawing exceeds 256 KiB"));
        }
        let actual = hex::encode(Sha256::digest(&bytes));
        let etag = headers
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(|s| {
                s.trim_start_matches("W/")
                    .trim_matches('"')
                    .to_ascii_lowercase()
            });
        if let Some(etag) = etag
            && etag != actual
        {
            return Err(unexpected(
                routes::DRAWING,
                &format!("the drawing's SHA-256 {actual} does not match its ETag {etag}"),
            ));
        }
        Ok(Some(Drawing {
            sha256: actual,
            bytes,
        }))
    }

    /// `DELETE /api/v1/drawings/current`.
    ///
    /// # Errors
    /// Server or transport errors.
    pub async fn delete_drawing(&self) -> Result<()> {
        let r = self.bearer(self.request(Method::DELETE, routes::DRAWING, true))?;
        self.empty(routes::DRAWING, r).await
    }

    /// `GET /api/v1/orgs/{org}/byo/deepgram`.
    ///
    /// # Errors
    /// Server or transport errors.
    pub async fn byo_deepgram(&self, org: &OrgId) -> Result<ByoStatus> {
        let path = routes::byo_deepgram(org.as_str());
        let r = self.bearer(self.request(Method::GET, &path, false))?;
        self.json(&path, r).await
    }

    /// `PUT /api/v1/orgs/{org}/byo/deepgram` (org owner or admin).
    ///
    /// # Errors
    /// `not_org_admin`, server or transport errors.
    pub async fn set_byo_deepgram(
        &self,
        org: &OrgId,
        request: &ByoDeepgramRequest,
    ) -> Result<ByoStatus> {
        let path = routes::byo_deepgram(org.as_str());
        let r = self.bearer(self.request(Method::PUT, &path, true).json(request))?;
        self.json(&path, r).await
    }

    /// `DELETE /api/v1/orgs/{org}/byo/deepgram` (org owner or admin).
    ///
    /// # Errors
    /// `not_org_admin`, server or transport errors.
    pub async fn delete_byo_deepgram(&self, org: &OrgId) -> Result<()> {
        let path = routes::byo_deepgram(org.as_str());
        let r = self.bearer(self.request(Method::DELETE, &path, true))?;
        self.empty(&path, r).await
    }

    /// `POST /api/v1/reports`; the bearer is sent when a session is attached.
    ///
    /// # Errors
    /// `invalid_input` for a malformed PR URL; server or transport errors.
    pub async fn report(
        &self,
        request: &ReportRequest,
        key: &IdempotencyKey,
    ) -> Result<ReportResponse> {
        if let Some(pr) = &request.pr
            && !ReportRequest::pr_is_valid(pr)
        {
            return Err(Error::invalid_input(format!(
                "--pr `{pr}` is not a pull request of teamofsilicons/silicon-peek"
            ))
            .with_hint("pass https://github.com/teamofsilicons/silicon-peek/pull/<number>"));
        }
        if request.message.trim().is_empty() {
            return Err(Error::invalid_input(
                "the report message is empty; describe what you ran, what happened and what you expected",
            ));
        }
        let mut r = self.post(routes::REPORTS, key, request, true);
        if self.session.is_some() {
            r = self.bearer(r)?;
        }
        self.json(routes::REPORTS, r).await
    }

    /// `POST /api/web/telemetry` (best effort; the CLI uses a 300 ms timeout).
    /// Does nothing when telemetry is off.
    ///
    /// # Errors
    /// `invalid_input` for an empty, oversized or > 40-event batch; server or
    /// transport errors.
    pub async fn telemetry(
        &self,
        batch: &TelemetryBatch,
        source: &str,
        timeout: Option<Duration>,
    ) -> Result<()> {
        if !self.telemetry {
            return Ok(());
        }
        if batch.events.is_empty() || batch.events.len() > 40 {
            return Err(Error::invalid_input(format!(
                "a telemetry batch carries 1–40 events, not {}",
                batch.events.len()
            )));
        }
        let body = serde_json::to_vec(batch)
            .map_err(|e| Error::internal(format!("serializing telemetry failed: {e}")))?;
        if body.len() > 64 * 1024 {
            return Err(Error::invalid_input(format!(
                "a telemetry batch is at most 64 KiB, this one is {} bytes",
                body.len()
            )));
        }
        let mut r = self
            .request(Method::POST, routes::WEB_TELEMETRY, false)
            .header(
                headers::IDEMPOTENCY_KEY,
                IdempotencyKey::generate().as_str(),
            )
            .header(headers::SOURCE, source)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body);
        if let Some(t) = timeout {
            r = r.timeout(t);
        }
        self.empty(routes::WEB_TELEMETRY, r).await
    }

    async fn json<T: DeserializeOwned>(&self, path: &str, r: RequestBuilder) -> Result<T> {
        let (status, headers, bytes) = self.send(path, r).await?;
        self.check(path, status, &headers, &bytes)?;
        decode(path, &bytes)
    }

    async fn empty(&self, path: &str, r: RequestBuilder) -> Result<()> {
        let (status, headers, bytes) = self.send(path, r).await?;
        self.check(path, status, &headers, &bytes)
    }

    async fn send(
        &self,
        path: &str,
        r: RequestBuilder,
    ) -> Result<(StatusCode, HeaderMap, Vec<u8>)> {
        let response = r.send().await.map_err(|e| self.transport(path, &e))?;
        self.drain(path, response).await
    }

    async fn drain(
        &self,
        path: &str,
        mut response: reqwest::Response,
    ) -> Result<(StatusCode, HeaderMap, Vec<u8>)> {
        let status = response.status();
        let headers = response.headers().clone();
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| self.transport(path, &e))?
        {
            if bytes.len() + chunk.len() > MAX_RESPONSE_BYTES {
                return Err(unexpected(path, "the response exceeds 16 MiB"));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok((status, headers, bytes))
    }

    fn check(
        &self,
        path: &str,
        status: StatusCode,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Result<()> {
        if status.is_success() {
            return Ok(());
        }
        Err(decode_error(&self.api, path, status, headers, body))
    }

    fn transport(&self, path: &str, e: &reqwest::Error) -> Error {
        let target = format!("{}{path}", self.api);
        let (message, hint) = if e.is_timeout() {
            (
                format!(
                    "the request to {target} timed out (limit {} s); whether it took effect is unknown",
                    self.timeout.as_secs()
                ),
                "retry the same command: peek reuses the idempotency key, so a retry cannot apply twice",
            )
        } else if e.is_connect() {
            (
                format!("could not connect to {target}: {}", root_cause(e)),
                "check the network and that --api / PEEK_API_URL points at a running peek-server",
            )
        } else {
            (
                format!("the request to {target} failed: {}", root_cause(e)),
                "retry; if it keeps failing, check https://backend.peek.teamofsilicons.com/readyz",
            )
        };
        Error::new(ErrorCode::BackendUnavailable, message)
            .with_hint(hint)
            .with_retryable(true)
            .with_origin(Origin::Transport)
    }
}

fn root_cause(e: &(dyn std::error::Error + 'static)) -> String {
    let mut cur: &(dyn std::error::Error + 'static) = e;
    while let Some(next) = cur.source() {
        cur = next;
    }
    cur.to_string()
}

fn decode<T: DeserializeOwned>(path: &str, bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|e| {
        unexpected(
            path,
            &format!("the response body does not match this peek's API types: {e}"),
        )
    })
}

fn unexpected(path: &str, why: &str) -> Error {
    Error::new(
        ErrorCode::UnexpectedResponse,
        format!("peek-server answered {path} with a response this peek cannot use: {why}"),
    )
    .with_hint("update peek (honeycomb update 'peek'); if that does not help, run peek report")
}

fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
}

/// Decodes a non-2xx response into an [`Error`] (exposed for peekd, which
/// talks to Deepgram with its own requests but reuses this mapping for
/// peek-server responses it proxies).
#[must_use]
pub fn decode_error(
    api: &ApiUrl,
    path: &str,
    status: StatusCode,
    headers: &HeaderMap,
    body: &[u8],
) -> Error {
    #[derive(serde::Deserialize)]
    struct Envelope {
        error: ErrorObject,
    }
    let header_request_id = headers
        .get(headers::REQUEST_ID)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    if let Ok(Envelope { error }) = serde_json::from_slice::<Envelope>(body) {
        let request_id = error.request_id.clone().or(header_request_id);
        return Error::from_object(error, Origin::Server)
            .with_status(status.as_u16())
            .with_request_id(request_id)
            .with_retry_after(retry_after(headers));
    }
    let (code, hint) = match status.as_u16() {
        400 | 422 => (
            ErrorCode::InvalidInput,
            "this looks like a peek bug; run peek report",
        ),
        401 => (
            ErrorCode::Unauthenticated,
            "log in again: peek login '<SLT>'",
        ),
        404 => (
            ErrorCode::NotFound,
            "check --api / PEEK_API_URL; the route does not exist there",
        ),
        413 => (ErrorCode::PayloadTooLarge, "send less data"),
        429 => (ErrorCode::RateLimited, "wait and retry"),
        408 | 500..=599 => (
            ErrorCode::BackendUnavailable,
            "retry later; peek-server health is at /readyz",
        ),
        _ => (
            ErrorCode::UnexpectedResponse,
            "update peek (honeycomb update 'peek'); if that does not help, run peek report",
        ),
    };
    let retryable = code.default_retryable();
    Error::new(
        code,
        format!("peek-server ({api}) answered {path} with HTTP {status} and no error details"),
    )
    .with_hint(hint)
    .with_retryable(retryable)
    .with_status(status.as_u16())
    .with_request_id(header_request_id)
    .with_retry_after(retry_after(headers))
    .with_origin(Origin::Server)
}
