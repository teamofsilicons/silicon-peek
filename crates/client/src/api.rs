//! Request and response types for every peek-server route (BLUEPRINT §5.2).
//!
//! Requests peek-server receives from peek clients are `deny_unknown_fields`
//! (the server refuses what it does not understand). Responses are decoded
//! leniently, so a newer server may add fields.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    Secret,
    identity::{AccountId, Actor, ApiUrl},
    timestamp::Timestamp,
};

/// Route paths, relative to the API origin.
pub mod routes {
    /// `GET` liveness.
    pub const HEALTHZ: &str = "/healthz";
    /// `GET` readiness.
    pub const READYZ: &str = "/readyz";
    /// `GET` public app discovery.
    pub const ACCOUNTS: &str = "/api/v1/accounts";
    /// `POST` SLT exchange.
    pub const LOGIN: &str = "/api/v1/auth/login";
    /// `POST` token rotation.
    pub const REFRESH: &str = "/api/v1/auth/refresh";
    /// `POST` revocation (and Ting grant removal when a bearer is sent).
    pub const LOGOUT: &str = "/api/v1/auth/logout";
    /// `GET` live-verified identity.
    pub const ME: &str = "/api/v1/auth/me";
    /// `POST` Ting recipient enrollment.
    pub const TING_RECIPIENT: &str = "/api/v1/ting/recipient";
    /// `POST` answer delivery.
    pub const DELIVERIES: &str = "/api/v1/deliveries";
    /// `POST` speech provider selection and token/proxy verdict.
    pub const SPEECH_TOKEN: &str = "/api/v1/speech/token";
    /// The speech proxy's base path (`{"mode":"proxy"}` token replies point
    /// `base_url` at `<public origin>` + this).
    pub const SPEECH_BASE: &str = "/api/v1/speech";
    /// `POST` STT through the speech proxy (raw WAV in, `SpeechTranscript` JSON out).
    pub const SPEECH_LISTEN: &str = "/api/v1/speech/listen";
    /// `PUT`/`GET`/`DELETE` the Silicon's drawing copy.
    pub const DRAWING: &str = "/api/v1/drawings/current";
    /// `POST` bug reports.
    pub const REPORTS: &str = "/api/v1/reports";
    /// `POST` telemetry gateway.
    pub const WEB_TELEMETRY: &str = "/api/web/telemetry";
    /// `POST` ACCOUNTS webhooks (HMAC-signed raw body).
    pub const ACCOUNTS_WEBHOOK: &str = "/webhooks/accounts";

    /// `GET`/`PUT`/`DELETE /api/v1/accounts/{account}/byo/deepgram`.
    #[must_use]
    pub fn byo_deepgram(account: &str) -> String {
        format!("/api/v1/accounts/{account}/byo/deepgram")
    }
}

/// Header names peek uses.
pub mod headers {
    /// Idempotency key on every POST.
    pub const IDEMPOTENCY_KEY: &str = "idempotency-key";
    /// Optional account consistency hint; bearer authority is authoritative.
    pub const ACCOUNT_ID: &str = "x-account-id";
    /// `on` or `off`; `off` makes the backend skip request events.
    pub const TELEMETRY: &str = "x-peek-telemetry";
    /// The caller's version.
    pub const CLIENT_VERSION: &str = "peek-client-version";
    /// Relayed telemetry source: `daemon`, `cli` or `mac`.
    pub const SOURCE: &str = "x-peek-source";
    /// Trace ID carried from the CLI.
    pub const TRACE_ID: &str = "x-peek-trace-id";
    /// Server request ID on every response.
    pub const REQUEST_ID: &str = "x-request-id";
    /// Drawing hash on `PUT /api/v1/drawings/current`.
    pub const DRAWING_SHA256: &str = "x-peek-drawing-sha256";
    /// Deepgram's request ID, passed through by the speech proxy.
    pub const DG_REQUEST_ID: &str = "dg-request-id";
    /// Deepgram's billed character count, passed through by the speech proxy.
    pub const DG_CHAR_COUNT: &str = "dg-char-count";
}

/// `GET /healthz`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    /// `ok`.
    pub status: String,
    /// `peek`.
    pub service: String,
    /// The server version.
    pub version: String,
}

/// `GET /readyz` checks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadyChecks {
    /// `ok` or an error summary.
    pub db: String,
    /// `ok` or `missing`.
    pub accounts_config: String,
    /// `ok` or `missing`.
    pub ting_config: String,
    /// Legacy Deepgram status.
    #[serde(default = "missing_provider_status")]
    pub deepgram: String,
    /// `ElevenLabs` speech synthesis: `configured` or `missing`.
    #[serde(default = "missing_provider_status")]
    pub elevenlabs: String,
    /// `OpenAI` transcription: `configured` or `missing` (absent on older servers).
    #[serde(default = "missing_provider_status")]
    pub openai: String,
}

fn missing_provider_status() -> String {
    "missing".to_owned()
}

/// `GET /readyz` (200 when ready, 503 otherwise, same body).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ready {
    /// `ready` or `not_ready`.
    pub status: String,
    /// Per-dependency checks.
    pub checks: ReadyChecks,
}

/// Version compatibility advertised by the backend.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Compatibility {
    /// Supported CLI versions, as a semver requirement.
    pub cli: String,
    /// Supported IPC protocol majors.
    pub ipc_protocols: Vec<u32>,
}

/// `GET /api/v1/accounts`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountsDiscovery {
    /// `peek`.
    pub app_id: String,
    /// `v1`.
    pub api_version: String,
    /// The backend origin.
    pub api_base_url: String,
    /// The ACCOUNTS origin.
    pub accounts_base_url: String,
    /// Compatibility ranges.
    pub compatibility: Compatibility,
}

/// `POST /api/v1/auth/login` body: exactly `{"slt":"…"}`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoginRequest {
    /// The single-use ACCOUNTS short-lived token.
    pub slt: Secret,
}

/// `POST /api/v1/auth/refresh` body: exactly `{"refresh_token":"sar_…"}`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RefreshRequest {
    /// The stored refresh token.
    pub refresh_token: Secret,
}

/// `POST /api/v1/auth/logout` body: `{"token":"sar_…"}`, plus
/// `"revoke_ting":true` only for `peek logout --revoke-ting`.
///
/// The Ting recipient grant belongs to the Silicon, not to one home: other
/// homes of the same Silicon (Stemcell plus a hand-run home) keep receiving
/// answers through it, so a plain logout leaves it alone. `revoke_ting` is
/// omitted when false, so a default logout stays `{"token"}` for any backend.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogoutRequest {
    /// The refresh token to revoke.
    pub token: Secret,
    /// Also revoke the Silicon's Ting recipient grant (needs the bearer).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub revoke_ting: bool,
}

/// Ting enrollment state reported by login, `me` and `ting enroll`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TingEnrollment {
    /// Whether the Silicon is an active Ting recipient for peek.
    pub subscribed: bool,
    /// The Ting subscription ID, when subscribed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_id: Option<String>,
    /// Why enrollment failed, when it did (the login still succeeds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<EnrollmentError>,
}

/// A transient enrollment failure.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrollmentError {
    /// Stable code.
    pub code: String,
    /// What failed.
    pub message: String,
}

/// The session returned by login and refresh. Only login carries `ting`.
/// The backend encrypts successful exchanges for safe lost-response recovery.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionResponse {
    /// JWT access token.
    pub access_token: Secret,
    /// `sar_…`.
    pub refresh_token: Secret,
    /// `Bearer`.
    pub token_type: String,
    /// Access token lifetime in seconds.
    pub expires_in: u64,
    /// Absolute expiry of the persistent refresh-token family.
    pub refresh_token_expires_at: Option<Timestamp>,
    /// Space-separated granted scopes.
    pub scope: String,
    /// The authenticated actor.
    pub actor: Actor,
    /// The selected account.
    pub account_id: AccountId,
    /// Every account the grant covers.
    #[serde(default)]
    pub account_ids: Vec<AccountId>,
    /// `peek:<account UUID>`.
    pub membership_id: String,
    /// Whether a required scope is missing (log in again with consent).
    #[serde(default)]
    pub reconsent_required: bool,
    /// The actor's display name, when disclosed.
    #[serde(default)]
    pub display_name: Option<String>,
    /// Ting enrollment (login only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ting: Option<TingEnrollment>,
}

impl SessionResponse {
    /// Validates an ordinary ACCOUNTS token pair and its single identity context.
    ///
    /// # Errors
    /// `unexpected_response` for malformed, unscoped or delegated credentials.
    pub fn validate(&self) -> crate::Result<()> {
        if self.token_type != "Bearer"
            || self.expires_in == 0
            || self.access_token.expose().split('.').count() != 3
            || !self.refresh_token.expose().starts_with("sar_")
            || self.actor.actor_type != self.actor.public_id.actor_type()
            || self.account_ids != [self.account_id.clone()]
            || self.membership_id != format!("peek:{}", self.account_id)
            || self
                .scope
                .split_ascii_whitespace()
                .any(|scope| scope.starts_with("obo:"))
        {
            return Err(crate::Error::new(
                crate::ErrorCode::UnexpectedResponse,
                "the backend did not return a valid ACCOUNTS session",
            ));
        }
        Ok(())
    }
}

/// `GET /api/v1/auth/me`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Me {
    /// Always `true` on 200.
    pub authenticated: bool,
    /// The introspected actor.
    pub actor: Actor,
    /// Display name (needs `profile`).
    #[serde(default)]
    pub display_name: Option<String>,
    /// The account from `X-Account-ID`.
    pub account_id: AccountId,
    /// `peek:<account UUID>`.
    pub membership_id: String,
    /// Granted scopes.
    #[serde(default)]
    pub scopes: Vec<String>,
    /// Whether a required scope is missing.
    #[serde(default)]
    pub reconsent_required: bool,
    /// Enrollment state from the backend's records.
    pub ting: TingEnrollment,
}

/// `POST /api/v1/ting/recipient` response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TingRecipient {
    /// `true` on success.
    pub subscribed: bool,
    /// The Ting subscription ID.
    pub subscription_id: String,
}

/// Delivery outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStatus {
    /// Ting accepted the ting.
    Accepted,
}

/// `POST /api/v1/deliveries` 200 response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryResponse {
    /// Echo of the request's event ID.
    pub event_id: String,
    /// Ting's message ID.
    pub ting_id: String,
    /// `accepted`.
    pub status: DeliveryStatus,
    /// Accepted but muted by the recipient: never delivered.
    #[serde(default)]
    pub silent: bool,
    /// Ting answered a same-key replay.
    #[serde(default)]
    pub replayed: bool,
}

/// What a speech token or proxy route is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeechPurpose {
    /// Text to speech (`ElevenLabs`).
    Tts,
    /// Speech to text (`OpenAI` gpt-transcribe).
    Stt,
}

/// `POST /api/v1/speech/token` body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpeechTokenRequest {
    /// TTS or STT.
    pub purpose: SpeechPurpose,
}

/// Which account serves the speech request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeySource {
    /// peek's own key.
    Peek,
    /// The account's BYO key.
    Account,
}

/// Request parameters every Deepgram call must carry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeechParams {
    /// Add `mip_opt_out=true`. peek always opts out of Deepgram's Model
    /// Improvement Program; clients add it even if this says `false`.
    pub mip_opt_out: bool,
    /// `tag=` values (e.g. `peek`, `production`).
    pub tags: Vec<String>,
}

/// Speech service used by this token or proxy route.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeechProvider {
    /// Deepgram (legacy servers; current clients refuse this provider).
    #[default]
    Deepgram,
    /// `ElevenLabs` v4 text to speech.
    Elevenlabs,
    /// `OpenAI` completed-recording transcription.
    Openai,
}

/// How peekd reaches the speech service for this token.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeechMode {
    /// Connect to Deepgram Voice Agent at `base_url` with a short-lived JWT
    /// in `Authorization: Bearer <access_token>`.
    #[default]
    Direct,
    /// Call peek-server's `OpenAI` STT proxy at `base_url` (`/listen`)
    /// with the Silicon's own session.
    Proxy,
}

/// `POST /api/v1/speech/token` response. A direct JWT lives 30 seconds
/// and is never logged. An established connection survives token expiry.
/// `OpenAI` STT proxy replies carry no provider credential.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpeechToken {
    /// Speech provider. Absent means Deepgram (older servers).
    #[serde(default)]
    pub provider: SpeechProvider,
    /// Direct (JWT) or proxy. Absent means direct (older servers).
    #[serde(default)]
    pub mode: SpeechMode,
    /// `Authorization: Bearer <JWT>` for Deepgram (direct mode only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_token: Option<Secret>,
    /// Seconds this answer (the JWT, or the proxy verdict) may be reused.
    pub expires_in: u64,
    /// Direct: `wss://agent.deepgram.com/v1/agent/converse`. Proxy:
    /// `<peek-server public origin>/api/v1/speech`.
    pub base_url: String,
    /// Which key serves the calls.
    pub key_source: KeySource,
    /// Parameters to add.
    pub params: SpeechParams,
}

/// `ElevenLabs` linear16 output uses a fixed sample rate of 24000 Hz.
pub const SPEAK_SAMPLE_RATES: [u32; 1] = [24_000];

/// Speech inputs for the direct `ElevenLabs` Voice Agent connection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpeechSpeakRequest {
    /// 1–2000 characters.
    pub text: String,
    /// `ElevenLabs` voice ID (for example `JBFqnCBsd6RMkjVDRZzb`); the wire field remains `model`.
    pub model: String,
    /// Speaking style, accent, pace and delivery instructions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_instructions: Option<String>,
    /// Optional BCP 47 language hint; absent lets `ElevenLabs` detect the language.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// linear16 sample rate (default 24000; one of [`SPEAK_SAMPLE_RATES`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
}

/// Provider-neutral result of `POST /api/v1/speech/listen`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeechTranscript {
    /// The transcribed text; empty when nothing was heard.
    pub text: String,
    /// Provider request ID, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// Language reported by the provider (never inferred from a request hint).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detected_language: Option<String>,
}

/// Query parameters accepted by `POST /api/v1/speech/listen`.
/// `detect_language` and `keyterm` may repeat. The server translates these
/// hints into `OpenAI` transcription fields; `smart_format` is kept for compatibility.
pub const LISTEN_PARAMS: [&str; 6] = [
    "model",
    "language",
    "detect_language",
    "keyterm",
    "numerals",
    "smart_format",
];

/// Largest audio body `POST /api/v1/speech/listen` accepts (4 MiB, the WAV
/// limit of IPC).
pub const LISTEN_MAX_BYTES: usize = 4 * 1024 * 1024;

/// `PUT /api/v1/drawings/current` response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DrawingStored {
    /// Hex SHA-256 of the script.
    pub sha256: String,
    /// Script size.
    pub bytes: u64,
    /// When the backend stored it.
    pub updated_at: Timestamp,
}

/// `GET /api/v1/drawings/current`: the script and its hash (from `ETag`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Drawing {
    /// Hex SHA-256 (the unquoted `ETag`).
    pub sha256: String,
    /// The JavaScript source.
    pub bytes: Vec<u8>,
}

/// `PUT /api/v1/accounts/{account}/byo/deepgram` body. The key is never returned.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ByoDeepgramRequest {
    /// A Deepgram key with the Member role.
    pub api_key: Secret,
    /// A Deepgram-compatible origin (default `https://api.deepgram.com`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

/// BYO key status.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ByoStatus {
    /// Whether the account has a key.
    pub configured: bool,
    /// When it was last changed.
    #[serde(default)]
    pub updated_at: Option<Timestamp>,
    /// Its base URL.
    #[serde(default)]
    pub base_url: Option<String>,
}

/// Context attached to a bug report. Never secrets or content.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportContext {
    /// The CLI version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_version: Option<String>,
    /// `macos-aarch64` etc.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    /// The command that failed (no arguments).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// The error code it failed with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

/// `POST /api/v1/reports` body.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportRequest {
    /// The report; its first line becomes the issue title.
    pub message: String,
    /// `https://github.com/teamofsilicons/silicon-peek/pull/<n>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<String>,
    /// Non-secret context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<ReportContext>,
    /// `peek doctor` output with `--attach-status` (non-secret).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<Value>,
}

impl ReportRequest {
    /// Whether `pr` is a pull request URL of the peek repository.
    #[must_use]
    pub fn pr_is_valid(pr: &str) -> bool {
        pr.strip_prefix("https://github.com/teamofsilicons/silicon-peek/pull/")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    }
}

/// Where a report ended up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportStatus {
    /// Filed as a GitHub issue.
    Filed,
    /// Stored for operators (GitHub unavailable).
    Stored,
}

/// `POST /api/v1/reports` response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportResponse {
    /// `rep_…`.
    pub id: String,
    /// Filed or stored.
    pub status: ReportStatus,
    /// The GitHub issue, when filed.
    #[serde(default)]
    pub issue_url: Option<String>,
}

/// One relayed telemetry event (the web SDK shape).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TelemetryEvent {
    /// Unique per event (dedup key).
    pub id: String,
    /// The event name.
    #[serde(rename = "type")]
    pub event_type: String,
    /// Event fields (the §6.4 envelope).
    pub data: Value,
    /// Metadata, including `occurred_at`.
    pub metadata: Value,
}

/// Tables the gateway accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TelemetryTable {
    /// CLI and daemon events.
    Peekclidaemon,
    /// Automatic analytics (web and Peek.app).
    Peekfrontendanalytics,
    /// Explicit events (web and Peek.app).
    Peekfrontendevents,
}

/// `POST /api/web/telemetry` body: 1–40 events, ≤ 64 KiB.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetryBatch {
    /// The destination table.
    pub table: TelemetryTable,
    /// The events.
    pub events: Vec<TelemetryEvent>,
}

/// The shape `peek accounts --json` prints: static, offline, no side effects
/// (BLUEPRINT §7.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountsInfo {
    /// `peek`.
    pub app_id: String,
    /// The developer's public Silicon Accounts identity.
    pub owner_id: String,
    /// `Peek`.
    pub name: String,
    /// This build.
    pub version: String,
    /// `v1`.
    pub api_version: String,
    /// The API origin in use.
    pub api_url: String,
    /// ACCOUNTS backend.
    pub accounts_url: String,
    /// ACCOUNTS consent UI.
    pub auth_url: String,
    /// `short_lived_token`.
    pub login_method: String,
    /// Always `false`: peek never issues ACCOUNTS credentials.
    pub credential_issuer: bool,
    /// How to log in.
    pub login: String,
    /// Docs.
    pub docs_url: String,
    /// Source.
    pub repository_url: String,
    /// The Rust client crate.
    pub rust_package: String,
    /// The CLI crate.
    pub cli_package: String,
    /// How to install.
    pub install: String,
    /// Platform support.
    pub platforms: Platforms,
}

/// Platform support tiers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Platforms {
    /// Everything works.
    pub full: Vec<String>,
    /// Only the ACCOUNTS contract (`accounts`, `login`, `status`, `logout`, `config`).
    pub accounts_only: Vec<String>,
}

impl AccountsInfo {
    /// The static discovery document for `api_url`.
    #[must_use]
    pub fn new(api_url: &ApiUrl) -> Self {
        let s = |v: &str| v.to_owned();
        Self {
            app_id: s(crate::APP_ID),
            owner_id: s(crate::OWNER_ACCOUNT),
            name: s("Peek"),
            version: s(crate::VERSION),
            api_version: s(crate::API_VERSION),
            api_url: api_url.as_str().to_owned(),
            accounts_url: s(crate::ACCOUNTS_URL),
            auth_url: s(crate::AUTH_URL),
            login_method: s("short_lived_token"),
            credential_issuer: false,
            login: s("Run `silicon-accounts login --app peek --json`, then `peek login '<SLT>'`."),
            docs_url: s(crate::DOCS_URL),
            repository_url: s(crate::REPOSITORY_URL),
            rust_package: s("silicon-peek-client"),
            cli_package: s("silicon-peek-cli"),
            install: s("apps install 'peek'"),
            platforms: Platforms {
                full: vec![s("macos-aarch64"), s("macos-x86_64")],
                accounts_only: vec![
                    s("linux-x86_64"),
                    s("linux-aarch64"),
                    s("windows-x86_64"),
                    s("windows-aarch64"),
                ],
            },
        }
    }
}
