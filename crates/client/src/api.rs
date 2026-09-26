//! Request and response types for every peek-server route (BLUEPRINT §5.2).
//!
//! Requests peek-server receives from peek clients are `deny_unknown_fields`
//! (the server refuses what it does not understand). Responses are decoded
//! leniently, so a newer server may add fields.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    Secret,
    identity::{Actor, ApiUrl, OrgId},
    timestamp::Timestamp,
};

/// Route paths, relative to the API origin.
pub mod routes {
    /// `GET` liveness.
    pub const HEALTHZ: &str = "/healthz";
    /// `GET` readiness.
    pub const READYZ: &str = "/readyz";
    /// `GET` discovery (the test key is optional).
    pub const IAM: &str = "/api/v1/iam";
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
    /// `POST` Deepgram JWT minting (or the proxy verdict).
    pub const SPEECH_TOKEN: &str = "/api/v1/speech/token";
    /// The speech proxy's base path (`{"mode":"proxy"}` token replies point
    /// `base_url` at `<public origin>` + this).
    pub const SPEECH_BASE: &str = "/api/v1/speech";
    /// `POST` TTS through the speech proxy (streams linear16 audio back).
    pub const SPEECH_SPEAK: &str = "/api/v1/speech/speak";
    /// `POST` STT through the speech proxy (raw audio in, Deepgram JSON out).
    pub const SPEECH_LISTEN: &str = "/api/v1/speech/listen";
    /// `PUT`/`GET`/`DELETE` the Silicon's drawing copy.
    pub const DRAWING: &str = "/api/v1/drawings/current";
    /// `POST` bug reports.
    pub const REPORTS: &str = "/api/v1/reports";
    /// `POST` telemetry gateway.
    pub const WEB_TELEMETRY: &str = "/api/web/telemetry";
    /// `POST` IAM webhooks (HMAC-signed raw body).
    pub const IAM_WEBHOOK: &str = "/webhooks/iam";

    /// `GET`/`PUT`/`DELETE /api/v1/orgs/{org}/byo/deepgram`.
    #[must_use]
    pub fn byo_deepgram(org: &str) -> String {
        format!("/api/v1/orgs/{org}/byo/deepgram")
    }

    /// `PUT`/`GET /internal/honeycomb/organizations/{org}/testing-environments/{env}/operations/{op}`.
    #[must_use]
    pub fn participant_operation(org: &str, environment: &str, operation: &str) -> String {
        format!(
            "/internal/honeycomb/organizations/{org}/testing-environments/{environment}/operations/{operation}"
        )
    }
}

/// Header names peek uses.
pub mod headers {
    /// Idempotency key on every POST.
    pub const IDEMPOTENCY_KEY: &str = "idempotency-key";
    /// Org selection for bearer routes (and the login org hint).
    pub const ORG_ID: &str = "x-org-id";
    /// The peek testing app secret, on every route in a testing context.
    pub const TESTING_KEY: &str = "x-testing-environment-key";
    /// The testing generation, on every mutation except login and refresh.
    pub const TESTING_GENERATION: &str = "x-testing-environment-generation";
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
    pub iam_config: String,
    /// `ok` or `missing`.
    pub ting_config: String,
    /// `configured` or `missing`.
    pub deepgram: String,
}

/// `GET /readyz` (200 when ready, 503 otherwise, same body).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ready {
    /// `ready` or `not_ready`.
    pub status: String,
    /// Per-dependency checks.
    pub checks: ReadyChecks,
}

/// A testing environment as the backend describes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestingEnvironment {
    /// The environment UUID.
    pub id: Uuid,
    /// Its display name (shown as `TEST · <name>`).
    pub name: String,
    /// Its current generation (advances on every clean).
    pub generation: u64,
}

/// Version compatibility advertised by the backend.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Compatibility {
    /// Supported CLI versions, as a semver requirement.
    pub cli: String,
    /// Supported IPC protocol majors.
    pub ipc_protocols: Vec<u32>,
}

/// `GET /api/v1/iam`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IamDiscovery {
    /// `peek`.
    pub app_id: String,
    /// `v1`.
    pub api_version: String,
    /// The backend origin.
    pub api_base_url: String,
    /// The IAM origin.
    pub iam_base_url: String,
    /// The testing environment the presented test key selects.
    #[serde(default)]
    pub testing_environment_id: Option<Uuid>,
    /// Its generation.
    #[serde(default)]
    pub testing_generation: Option<u64>,
    /// Its description.
    #[serde(default)]
    pub testing_environment: Option<TestingEnvironment>,
    /// Compatibility ranges.
    pub compatibility: Compatibility,
}

/// `POST /api/v1/auth/login` body: exactly `{"slt":"…"}`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoginRequest {
    /// The single-use IAM short-lived token.
    pub slt: Secret,
}

/// `POST /api/v1/auth/refresh` body: exactly `{"refresh_token":"ort_…"}`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RefreshRequest {
    /// The stored refresh token.
    pub refresh_token: Secret,
}

/// `POST /api/v1/auth/logout` body: exactly `{"token":"ort_…"}`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogoutRequest {
    /// The refresh token to revoke.
    pub token: Secret,
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
/// The backend stores none of these tokens.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionResponse {
    /// `oat_…`.
    pub access_token: Secret,
    /// `ort_…`.
    pub refresh_token: Secret,
    /// `Bearer`.
    pub token_type: String,
    /// Access token lifetime in seconds.
    pub expires_in: u64,
    /// Space-separated granted scopes.
    pub scope: String,
    /// The authenticated actor.
    pub actor: Actor,
    /// The selected org.
    pub org_id: OrgId,
    /// Every org the grant covers.
    #[serde(default)]
    pub org_ids: Vec<OrgId>,
    /// `si:<handle>[<org>]`.
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
    /// The testing environment, in a testing context.
    #[serde(default)]
    pub testing_environment: Option<TestingEnvironment>,
}

/// `GET /api/v1/auth/me`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Me {
    /// Always `true` on 200.
    pub authenticated: bool,
    /// The introspected actor.
    pub actor: Actor,
    /// Display name (needs `self.profile.read`).
    #[serde(default)]
    pub display_name: Option<String>,
    /// The org from `X-Org-ID`.
    pub org_id: OrgId,
    /// `si:<handle>[<org>]`.
    pub membership_id: String,
    /// `owner`, `admin`, `member`… (needs `self.membership.read`).
    #[serde(default)]
    pub org_role: Option<String>,
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

/// What a Deepgram token is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeechPurpose {
    /// Text to speech (Aura-2).
    Tts,
    /// Speech to text (Nova-3).
    Stt,
}

/// `POST /api/v1/speech/token` body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpeechTokenRequest {
    /// TTS or STT.
    pub purpose: SpeechPurpose,
}

/// Which Deepgram key minted the token.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeySource {
    /// peek's own key.
    Peek,
    /// The org's BYO key.
    Org,
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

/// How peekd reaches Deepgram for this token.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeechMode {
    /// Call Deepgram at `base_url` with `Authorization: Bearer <access_token>`
    /// (a JWT minted with `POST /v1/auth/grant`).
    #[default]
    Direct,
    /// The Deepgram key cannot mint JWTs: call peek-server's speech proxy at
    /// `base_url` (`/speak`, `/listen`) with the Silicon's own session.
    Proxy,
}

/// `POST /api/v1/speech/token` response. A direct token's JWT lives ≤ 60 s
/// and is never logged; a proxy reply carries no credential at all.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpeechToken {
    /// Direct (JWT) or proxy. Absent means direct (older servers).
    #[serde(default)]
    pub mode: SpeechMode,
    /// `Authorization: Bearer <JWT>` for Deepgram (direct mode only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_token: Option<Secret>,
    /// Seconds this answer (the JWT, or the proxy verdict) may be reused.
    pub expires_in: u64,
    /// Direct: `https://api.deepgram.com` (or the org's base URL). Proxy:
    /// `<peek-server public origin>/api/v1/speech`.
    pub base_url: String,
    /// Which key serves the calls.
    pub key_source: KeySource,
    /// Parameters to add.
    pub params: SpeechParams,
}

/// Aura-2 output sample rates the speech proxy accepts (linear16).
pub const SPEAK_SAMPLE_RATES: [u32; 5] = [8_000, 16_000, 24_000, 32_000, 48_000];

/// `POST /api/v1/speech/speak` body (speech proxy).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpeechSpeakRequest {
    /// 1–2000 characters.
    pub text: String,
    /// `aura-2-<name>-<lang>`.
    pub model: String,
    /// linear16 sample rate (default 24000; one of [`SPEAK_SAMPLE_RATES`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
}

/// Query parameters `POST /api/v1/speech/listen` forwards to Deepgram; any
/// other parameter is refused. `detect_language` and `keyterm` may repeat.
/// peek-server adds `tag` and `mip_opt_out` itself.
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

/// `PUT /api/v1/orgs/{org}/byo/deepgram` body. The key is never returned.
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
    /// Whether the org has a key.
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

/// Honeycomb lifecycle participant contract (§2.9).
pub mod participant {
    use serde::{Deserialize, Serialize};
    use serde_json::Value;
    use uuid::Uuid;

    use crate::Secret;

    /// Lifecycle actions.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "kebab-case")]
    pub enum Action {
        /// Create the context.
        Prepare,
        /// Import data.
        Import,
        /// Re-import.
        RefreshImport,
        /// Rotate the root key (advances `key_version`).
        RotateKey,
        /// Wipe data (advances `generation`).
        Clean,
        /// Disable (Honeycomb "delete").
        Disable,
        /// Re-enable.
        Restore,
        /// Destroy, keeping a secret-free tombstone.
        Purge,
        /// Retire applications.
        RetireApplications,
    }

    /// `PUT …/operations/{operation_id}` body. Only these keys are sent.
    #[derive(Clone, Debug, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct OperationRequest {
        /// Must equal the route's operation ID.
        pub operation_id: Uuid,
        /// Must equal the route's environment ID.
        pub environment_id: Uuid,
        /// Must equal the route's org.
        pub org_id: String,
        /// Must be `peek`.
        pub app_id: String,
        /// Positive; advances.
        pub environment_revision: u64,
        /// Positive.
        pub generation: u64,
        /// Positive.
        pub key_version: u64,
        /// The action.
        pub action: Action,
        /// The environment root key (32 alphanumerics), when sent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub testing_key: Option<Secret>,
        /// Import snapshot, when sent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub snapshot: Option<Value>,
        /// Human reason, when sent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub reason: Option<String>,
        /// Applications retired by `retire-applications`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub retired_apps: Option<Vec<String>>,
    }

    /// Receipt state.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum ReceiptState {
        /// Barrier committed, work in progress.
        Pending,
        /// Done.
        Completed,
        /// Failed permanently.
        Failed,
    }

    /// The durable receipt (≤ 64 KiB; never keys, snapshots or error details).
    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct Receipt {
        /// State.
        pub state: ReceiptState,
        /// Echo.
        pub operation_id: Uuid,
        /// Echo.
        pub environment_id: Uuid,
        /// `peek`.
        pub app_id: String,
        /// Echo.
        pub environment_revision: u64,
        /// Echo.
        pub generation: u64,
        /// Echo.
        pub key_version: u64,
        /// Echo for `retire-applications`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub retired_apps: Option<Vec<String>>,
    }
}

/// The shape `peek iam --json` prints: static, offline, no side effects
/// (BLUEPRINT §7.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IamInfo {
    /// `peek`.
    pub app_id: String,
    /// `tos`.
    pub org_id: String,
    /// `Peek`.
    pub name: String,
    /// This build.
    pub version: String,
    /// `v1`.
    pub api_version: String,
    /// The API origin in use.
    pub api_url: String,
    /// IAM backend.
    pub iam_url: String,
    /// IAM consent UI.
    pub auth_url: String,
    /// `short_lived_token`.
    pub login_method: String,
    /// Always `false`: peek never issues IAM credentials.
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
    /// Added with `--test`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub testing: Option<IamTesting>,
}

/// `peek iam --json --test …`'s `testing` object (BLUEPRINT §7.3):
/// `{"environment_id","name","generation"}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IamTesting {
    /// The environment UUID (hyphenated).
    pub environment_id: Uuid,
    /// Its display name.
    pub name: String,
    /// Its current generation.
    pub generation: u64,
}

impl From<&TestingEnvironment> for IamTesting {
    fn from(t: &TestingEnvironment) -> Self {
        Self {
            environment_id: t.id,
            name: t.name.clone(),
            generation: t.generation,
        }
    }
}

/// Platform support tiers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Platforms {
    /// Everything works.
    pub full: Vec<String>,
    /// Only the IAM contract (`iam`, `login`, `status`, `logout`, `config`).
    pub iam_only: Vec<String>,
}

impl IamInfo {
    /// The static discovery document for `api_url`.
    #[must_use]
    pub fn new(api_url: &ApiUrl) -> Self {
        let s = |v: &str| v.to_owned();
        Self {
            app_id: s(crate::APP_ID),
            org_id: s(crate::OWNER_ORG),
            name: s("Peek"),
            version: s(crate::VERSION),
            api_version: s(crate::API_VERSION),
            api_url: api_url.as_str().to_owned(),
            iam_url: s(crate::IAM_URL),
            auth_url: s(crate::AUTH_URL),
            login_method: s("short_lived_token"),
            credential_issuer: false,
            login: s(
                "Mint an SLT with `iam silicon-login --app-id peek --grant-org <org> --approve-scopes` (Silicon) or `iam login --app-id peek --grant-org <org>` (Carbon), then run `peek login '<SLT>'`.",
            ),
            docs_url: s(crate::DOCS_URL),
            repository_url: s(crate::REPOSITORY_URL),
            rust_package: s("silicon-peek-client"),
            cli_package: s("silicon-peek-cli"),
            install: s("honeycomb install 'peek'"),
            platforms: Platforms {
                full: vec![s("macos-aarch64"), s("macos-x86_64")],
                iam_only: vec![
                    s("linux-x86_64"),
                    s("linux-aarch64"),
                    s("windows-x86_64"),
                    s("windows-aarch64"),
                ],
            },
            testing: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_bodies_are_exact_and_strict() -> Result<(), serde_json::Error> {
        let login = LoginRequest {
            slt: Secret::new("oac_x"),
        };
        assert_eq!(serde_json::to_string(&login)?, r#"{"slt":"oac_x"}"#);
        assert!(serde_json::from_value::<LoginRequest>(json!({"slt":"a","org":"b"})).is_err());
        let r = RefreshRequest {
            refresh_token: Secret::new("ort_x"),
        };
        assert_eq!(serde_json::to_string(&r)?, r#"{"refresh_token":"ort_x"}"#);
        let l = LogoutRequest {
            token: Secret::new("ort_x"),
        };
        assert_eq!(serde_json::to_string(&l)?, r#"{"token":"ort_x"}"#);
        let s = SpeechTokenRequest {
            purpose: SpeechPurpose::Stt,
        };
        assert_eq!(serde_json::to_string(&s)?, r#"{"purpose":"stt"}"#);
        Ok(())
    }

    #[test]
    fn session_response_decodes_the_blueprint_example() -> Result<(), serde_json::Error> {
        let s: SessionResponse = serde_json::from_value(json!({
            "access_token":"oat_a","refresh_token":"ort_b","token_type":"Bearer","expires_in":1800,
            "scope":"obo:ting:tings.send self.identity.read","actor":{"type":"silicon","public_id":"si:cleanup"},
            "org_id":"tos","org_ids":["tos"],"membership_id":"si:cleanup[tos]","reconsent_required":false,
            "display_name":"Cleanup","ting":{"subscribed":true,"subscription_id":"sub_1"},"testing_environment":null,
            "future_field":1}))?;
        assert_eq!(s.access_token.expose(), "oat_a");
        assert_eq!(s.ting.map(|t| t.subscribed), Some(true));
        let refreshed: SessionResponse = serde_json::from_value(json!({
            "access_token":"oat_c","refresh_token":"ort_d","token_type":"Bearer","expires_in":1800,
            "scope":"","actor":{"type":"silicon","public_id":"si:cleanup"},"org_id":"tos","membership_id":"si:cleanup[tos]"}))?;
        assert!(refreshed.ting.is_none());
        Ok(())
    }

    #[test]
    fn speech_tokens_in_both_modes() -> Result<(), serde_json::Error> {
        let direct: SpeechToken = serde_json::from_value(json!({
            "access_token":"jwt","expires_in":60,"base_url":"https://api.deepgram.com",
            "key_source":"peek","params":{"mip_opt_out":true,"tags":["peek","production"]}}))?;
        assert_eq!(direct.mode, SpeechMode::Direct);
        assert_eq!(
            direct.access_token.as_ref().map(Secret::expose),
            Some("jwt")
        );
        let proxy: SpeechToken = serde_json::from_value(json!({
            "mode":"proxy","expires_in":600,"base_url":"http://127.0.0.1:8080/api/v1/speech",
            "key_source":"org","params":{"mip_opt_out":true,"tags":["peek","testing"]}}))?;
        assert_eq!(proxy.mode, SpeechMode::Proxy);
        assert!(proxy.access_token.is_none());
        let v = serde_json::to_value(&proxy)?;
        assert!(v.get("access_token").is_none());
        assert_eq!(v["mode"], "proxy");
        let speak = SpeechSpeakRequest {
            text: "hi".into(),
            model: "aura-2-thalia-en".into(),
            sample_rate: None,
        };
        assert_eq!(
            serde_json::to_string(&speak)?,
            r#"{"text":"hi","model":"aura-2-thalia-en"}"#
        );
        assert!(
            serde_json::from_value::<SpeechSpeakRequest>(json!({"text":"a","model":"m","voice":1}))
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn iam_testing_is_the_section_7_3_shape() -> Result<(), serde_json::Error> {
        let env = TestingEnvironment {
            id: Uuid::nil(),
            name: "peek testing".into(),
            generation: 3,
        };
        let mut info = IamInfo::new(&ApiUrl::production());
        info.testing = Some(IamTesting::from(&env));
        let v = serde_json::to_value(&info)?;
        assert_eq!(
            v["testing"],
            json!({"environment_id": Uuid::nil(), "name": "peek testing", "generation": 3})
        );
        assert_eq!(v["version"], crate::VERSION);
        Ok(())
    }

    #[test]
    fn pr_urls() {
        assert!(ReportRequest::pr_is_valid(
            "https://github.com/teamofsilicons/silicon-peek/pull/12"
        ));
        assert!(!ReportRequest::pr_is_valid(
            "https://github.com/teamofsilicons/silicon-peek/pull/"
        ));
        assert!(!ReportRequest::pr_is_valid(
            "https://github.com/other/silicon-peek/pull/1"
        ));
        assert!(!ReportRequest::pr_is_valid(
            "https://github.com/teamofsilicons/silicon-peek/pull/1/files"
        ));
    }

    #[test]
    fn iam_info_matches_the_contract() -> Result<(), serde_json::Error> {
        let v = serde_json::to_value(IamInfo::new(&ApiUrl::production()))?;
        assert_eq!(v["app_id"], "peek");
        assert_eq!(v["org_id"], "tos");
        assert_eq!(v["credential_issuer"], false);
        assert_eq!(v["api_url"], "https://backend.peek.teamofsilicons.com");
        assert_eq!(
            v["platforms"]["full"],
            json!(["macos-aarch64", "macos-x86_64"])
        );
        assert!(v.get("testing").is_none());
        Ok(())
    }

    #[test]
    fn participant_bodies() -> Result<(), serde_json::Error> {
        let op: participant::OperationRequest = serde_json::from_value(json!({
            "operation_id": Uuid::now_v7(), "environment_id": Uuid::now_v7(), "org_id":"tos","app_id":"peek",
            "environment_revision":1,"generation":1,"key_version":1,"action":"refresh-import"}))?;
        assert_eq!(op.action, participant::Action::RefreshImport);
        assert!(
            serde_json::from_value::<participant::OperationRequest>(
                json!({"operation_id":Uuid::now_v7(),"bogus":1})
            )
            .is_err()
        );
        Ok(())
    }
}
