//! The error model shared by the CLI, peekd, Peek.app (over IPC) and peek-server.
//!
//! Every failure carries a stable machine code ([`ErrorCode`]), a message that
//! says exactly what failed and why, an optional hint that says how to fix it,
//! and whether retrying the same request can succeed. The wire shape is
//! [`ErrorObject`]:
//!
//! ```json
//! {"error":{"code":"side_taken","message":"…","hint":"…","retryable":false,
//!  "request_id":null,"details":{"owner":"si:dj","free":[1,4,6,7]}}}
//! ```
//!
//! [`ErrorCode::exit_code`] maps each code to the CLI exit code (BLUEPRINT
//! §7.5): 0 ok · 1 internal · 2 usage/invalid input · 3 not authenticated ·
//! 4 refused/precondition · 5 unavailable/transport.

use std::{error::Error as StdError, fmt, time::Duration};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Value, json};

/// Result alias for this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Process exit codes (BLUEPRINT D21).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExitCode {
    /// 0: success, including `login status` answering `authenticated:false`.
    Success,
    /// 1: an internal or unexpected failure (a bug, or a corrupt store).
    Internal,
    /// 2: usage error or invalid input; fix the command line or the JSON.
    Usage,
    /// 3: not authenticated; log in again.
    NotAuthenticated,
    /// 4: refused, or a precondition is not met.
    Refused,
    /// 5: a dependency is unavailable or the transport failed; retry later.
    Unavailable,
}

impl ExitCode {
    /// The numeric process exit status.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Success => 0,
            Self::Internal => 1,
            Self::Usage => 2,
            Self::NotAuthenticated => 3,
            Self::Refused => 4,
            Self::Unavailable => 5,
        }
    }

    /// The exit code implied by an HTTP status when the error code is unknown.
    #[must_use]
    pub const fn from_http_status(status: u16) -> Self {
        match status {
            401 => Self::NotAuthenticated,
            403 | 404 | 409 | 412 | 423 => Self::Refused,
            400 | 405 | 411 | 413 | 414 | 415 | 422 => Self::Usage,
            408 | 425 | 429 | 500..=599 => Self::Unavailable,
            _ => Self::Internal,
        }
    }
}

impl From<ExitCode> for std::process::ExitCode {
    fn from(value: ExitCode) -> Self {
        Self::from(value.code())
    }
}

macro_rules! error_codes {
    ($( $(#[$doc:meta])* $variant:ident = $name:literal => $exit:ident, $retry:literal; )*) => {
        /// Every error code peek emits, plus [`ErrorCode::Other`] for codes this
        /// build does not know (a newer server or daemon may add codes).
        #[derive(Clone, Debug, PartialEq, Eq, Hash)]
        #[non_exhaustive]
        pub enum ErrorCode {
            $( $(#[$doc])* $variant, )*
            /// A code this build does not recognise, preserved verbatim.
            Other(String),
        }

        impl ErrorCode {
            /// Every known code, in declaration order.
            pub const ALL: &'static [ErrorCode] = &[$(ErrorCode::$variant,)*];

            /// The wire spelling.
            #[must_use]
            pub fn as_str(&self) -> &str {
                match self {
                    $( Self::$variant => $name, )*
                    Self::Other(s) => s.as_str(),
                }
            }

            /// Parses a wire code. Unknown codes become [`ErrorCode::Other`].
            #[must_use]
            pub fn parse(s: &str) -> Self {
                match s {
                    $( $name => Self::$variant, )*
                    other => Self::Other(other.to_owned()),
                }
            }

            /// The CLI exit code for this error (BLUEPRINT §7.5). Unknown codes
            /// map to [`ExitCode::Internal`]; [`Error::exit_code`] refines that
            /// with the HTTP status when there is one.
            #[must_use]
            pub fn exit_code(&self) -> ExitCode {
                match self {
                    $( Self::$variant => ExitCode::$exit, )*
                    Self::Other(_) => ExitCode::Internal,
                }
            }

            /// Whether an error with this code is retryable by default.
            #[must_use]
            pub fn default_retryable(&self) -> bool {
                match self {
                    $( Self::$variant => $retry, )*
                    Self::Other(_) => false,
                }
            }
        }
    };
}

error_codes! {
    // exit 1: internal / unexpected
    /// An unexpected failure inside peek (a bug).
    InternalError = "internal_error" => Internal, false;
    /// A store file under `$SILICON_HOME/.peek` could not be parsed.
    StoreCorrupt = "store_corrupt" => Internal, false;
    /// peek-server or peekd answered with a shape this build cannot decode.
    UnexpectedResponse = "unexpected_response" => Internal, false;
    /// Ting saw a different body under an existing key (a bug in peek).
    TingKeyConflict = "ting_key_conflict" => Internal, false;
    /// Ting refused a proof or context (a bug in peek).
    TingRejected = "ting_rejected" => Internal, false;

    // exit 2: usage / invalid input
    /// Input failed validation.
    InvalidInput = "invalid_input" => Usage, false;
    /// Input is not valid JSON, or has duplicate keys.
    InvalidJson = "invalid_json" => Usage, false;
    /// Two flags that cannot be combined were given.
    ConflictingFlags = "conflicting_flags" => Usage, false;
    /// `peek send` without `--speak`, `--show` or `--ask`.
    NothingToSend = "nothing_to_send" => Usage, false;
    /// A text field exceeds its character limit.
    TextTooLong = "text_too_long" => Usage, false;
    /// An image caption exceeds 50 characters.
    CaptionTooLong = "caption_too_long" => Usage, false;
    /// An ask question exceeds 80 characters.
    QuestionTooLong = "question_too_long" => Usage, false;
    /// `--speak` exceeds 2000 characters.
    SpeakTooLong = "speak_too_long" => Usage, false;
    /// A show has more than 3 elements.
    TooManyElements = "too_many_elements" => Usage, false;
    /// A choice ask has more than 6 options.
    TooManyOptions = "too_many_options" => Usage, false;
    /// An image path could not be read.
    ImageUnreadable = "image_unreadable" => Usage, false;
    /// An image exceeds 10 MiB.
    ImageTooLarge = "image_too_large" => Usage, false;
    /// An image is not PNG, JPEG, HEIC, WebP or GIF.
    ImageUnsupported = "image_unsupported" => Usage, false;
    /// A drawing exceeds 256 KiB.
    DrawingTooLarge = "drawing_too_large" => Usage, false;
    /// `peek config set` named a key that does not exist.
    UnknownConfigKey = "unknown_config_key" => Usage, false;
    /// `SILICON_HOME` is empty, missing or not a usable directory.
    InvalidSiliconHome = "invalid_silicon_home" => Usage, false;
    /// A public ID was passed where an SLT is required (production only).
    SltIsPublicId = "slt_is_public_id" => Usage, false;
    /// A POST reached peek-server without an `Idempotency-Key`.
    IdempotencyKeyRequired = "idempotency_key_required" => Usage, false;
    /// An idempotency key was reused for a different request body.
    IdempotencyConflict = "idempotency_conflict" => Usage, false;
    /// An IPC frame exceeds a framing limit.
    FrameTooLarge = "frame_too_large" => Usage, false;
    /// A request body exceeds the server's limit.
    PayloadTooLarge = "payload_too_large" => Usage, false;
    /// Space Station refused the relayed telemetry batch.
    TelemetryRejected = "telemetry_rejected" => Usage, false;

    // exit 3: not authenticated
    /// No session for this home, API and context.
    NotLoggedIn = "not_logged_in" => NotAuthenticated, false;
    /// IAM rejected the session (terminal until the next login).
    SessionRejected = "session_rejected" => NotAuthenticated, false;
    /// IAM rejected the SLT (expired, used or invalid).
    SltRejected = "slt_rejected" => NotAuthenticated, false;
    /// `peek login --recover` ran after the 10-minute replay window.
    LoginAttemptExpired = "login_attempt_expired" => NotAuthenticated, false;
    /// The session lacks a scope peek needs; log in again to re-consent.
    ReconsentRequired = "reconsent_required" => NotAuthenticated, false;
    /// The testing app secret is malformed or was rejected.
    TestingSecretInvalid = "testing_secret_invalid" => NotAuthenticated, false;
    /// The testing environment's generation changed (it was cleaned).
    TestingGenerationChanged = "testing_generation_changed" => NotAuthenticated, false;
    /// peek-server did not accept the bearer token.
    Unauthenticated = "unauthenticated" => NotAuthenticated, false;
    /// The presented home token does not match `<home>/daemon-token`.
    HomeTokenMismatch = "home_token_mismatch" => NotAuthenticated, false;
    /// A delivery needs a fresh login or re-consent before it can proceed.
    AuthorityRequired = "authority_required" => NotAuthenticated, false;

    // exit 4: refused / precondition
    /// The Silicon has no position; run `peek register side <1-8>`.
    SideNotRegistered = "side_not_registered" => Refused, false;
    /// The Silicon has no drawing; run `peek register drawing <FILE.js>`.
    DrawingNotRegistered = "drawing_not_registered" => Refused, false;
    /// The position is held by another Silicon.
    SideTaken = "side_taken" => Refused, false;
    /// The drawing failed validation.
    DrawingInvalid = "drawing_invalid" => Refused, false;
    /// Deprecated alias of `queue_full`: sent only to CLIs older than 0.1.2
    /// (and by peekd 0.1.1).
    SlotBusy = "slot_busy" => Refused, false;
    /// The Silicon's queue already has one send on screen and five waiting.
    QueueFull = "queue_full" => Refused, true;
    /// No send with that ID belongs to this Silicon.
    SendNotFound = "send_not_found" => Refused, false;
    /// No scheduled send with that ID belongs to this Silicon.
    ScheduleNotFound = "schedule_not_found" => Refused, false;
    /// 500 sends are already scheduled for this Silicon.
    ScheduleFull = "schedule_full" => Refused, false;
    /// No ask with that ID belongs to this Silicon.
    AskNotFound = "ask_not_found" => Refused, false;
    /// The command needs macOS.
    PlatformUnsupported = "platform_unsupported" => Refused, false;
    /// peekd no longer speaks this CLI's protocol; update the CLI.
    CliOutdated = "cli_outdated" => Refused, false;
    /// A store file was written by a newer peek.
    StoreSchemaNewer = "store_schema_newer" => Refused, false;
    /// The private IAM app requires the org's membership.
    PrivateApplicationOrganizationRequired = "private_application_organization_required" => Refused, false;
    /// The Silicon is not enrolled as a Ting recipient for peek.
    RecipientNotRegistered = "recipient_not_registered" => Refused, false;
    /// The action needs org owner or admin.
    NotOrgAdmin = "not_org_admin" => Refused, false;
    /// peekd does not know the requested IPC op.
    UnknownOp = "unknown_op" => Refused, false;
    /// Another peekd already holds the instance lock.
    DaemonRunning = "daemon_running" => Refused, false;
    /// The peekd socket belongs to another operating-system user.
    DaemonIdentityMismatch = "daemon_identity_mismatch" => Refused, false;
    /// peek-server holds no drawing for this Silicon.
    DrawingNotFound = "drawing_not_found" => Refused, false;
    /// The testing environment has not been prepared for peek.
    EnvironmentNotPrepared = "environment_not_prepared" => Refused, false;
    /// The idempotent replay window (10 minutes) has passed.
    IdempotencyResponseExpired = "idempotency_response_expired" => Refused, false;
    /// The requested resource does not exist.
    NotFound = "not_found" => Refused, false;
    /// A testing-environment lifecycle operation conflicts with another one.
    LifecycleConflict = "lifecycle_conflict" => Refused, false;
    /// The request's `Origin` is not an allowed web origin.
    OriginNotAllowed = "origin_not_allowed" => Refused, false;
    /// The telemetry table is not accepted by this gateway.
    TelemetryTableUnavailable = "telemetry_table_unavailable" => Refused, false;

    // exit 5: unavailable / transport
    /// peek-server could not be reached or failed.
    BackendUnavailable = "backend_unavailable" => Unavailable, true;
    /// IAM could not be reached or failed.
    IamUnavailable = "iam_unavailable" => Unavailable, true;
    /// peek-server's IAM app credentials are missing or invalid (operator issue).
    IamMisconfigured = "iam_misconfigured" => Unavailable, true;
    /// peekd is not running or did not answer.
    DaemonUnavailable = "daemon_unavailable" => Unavailable, true;
    /// Peek.app and peekd could not be started.
    PeekServiceUnavailable = "peek_service_unavailable" => Unavailable, true;
    /// No GUI (Aqua) session is available to start Peek.app.
    NoGuiSession = "no_gui_session" => Unavailable, false;
    /// A Peek.app update is pending before peekd can serve this CLI.
    AppUpdatePending = "app_update_pending" => Unavailable, true;
    /// Speech is unavailable (no provider key, quota or provider outage).
    SpeechUnavailable = "speech_unavailable" => Unavailable, true;
    /// Ting could not be reached or is overloaded.
    TingUnavailable = "ting_unavailable" => Unavailable, true;
    /// A peek Ting type is not registered in this context (operator issue).
    TingTypeMissing = "ting_type_missing" => Unavailable, true;
    /// The same idempotent request is still executing.
    IdempotencyInProgress = "idempotency_in_progress" => Unavailable, true;
    /// Too many requests; honour `Retry-After`.
    RateLimited = "rate_limited" => Unavailable, true;
    /// The IPC peer violated the framing or envelope protocol.
    ProtocolError = "protocol_error" => Unavailable, false;
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for ErrorCode {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ErrorCode {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(Self::parse(&s))
    }
}

/// The wire error object: the body of `{"error":{…}}` on HTTP and the `error`
/// field of an IPC reply.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ErrorObject {
    /// Stable machine code.
    pub code: ErrorCode,
    /// What failed and why.
    pub message: String,
    /// How to fix it, when there is a known fix.
    #[serde(default)]
    pub hint: Option<String>,
    /// Whether retrying the same request can succeed.
    #[serde(default)]
    pub retryable: bool,
    /// Server request ID for support, when the error came from peek-server.
    #[serde(default)]
    pub request_id: Option<String>,
    /// Structured context (for example `{"owner":"si:dj","free":[1,4]}`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

/// Where an [`Error`] was produced.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Origin {
    /// Local validation or I/O in this process.
    Local,
    /// peek-server answered with an error status.
    Server,
    /// peekd (or Peek.app) answered with an error reply.
    Daemon,
    /// The request may not have reached its destination (connect, timeout, reset).
    Transport,
}

/// A peek failure. Build one with [`Error::new`] and the `with_*` methods.
///
/// Boxed, so `Result<T, Error>` stays one pointer wide on the error path.
#[derive(Debug)]
pub struct Error(Box<Inner>);

#[derive(Debug)]
struct Inner {
    code: ErrorCode,
    message: String,
    hint: Option<String>,
    retryable: bool,
    request_id: Option<String>,
    details: Option<Value>,
    status: Option<u16>,
    retry_after: Option<Duration>,
    origin: Origin,
    source: Option<Box<dyn StdError + Send + Sync + 'static>>,
}

impl Error {
    /// A local error with the code's default retryability.
    #[must_use]
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        let retryable = code.default_retryable();
        Self(Box::new(Inner {
            code,
            message: message.into(),
            hint: None,
            retryable,
            request_id: None,
            details: None,
            status: None,
            retry_after: None,
            origin: Origin::Local,
            source: None,
        }))
    }

    /// `invalid_input` with a message.
    #[must_use]
    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidInput, message)
    }

    /// `internal_error` with a message and the standard bug-report hint.
    #[must_use]
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InternalError, message)
            .with_hint("this is a bug in peek; report it with `peek report \"<what you ran>\"`")
    }

    /// Rebuilds an error received on the wire.
    #[must_use]
    pub fn from_object(object: ErrorObject, origin: Origin) -> Self {
        Self(Box::new(Inner {
            code: object.code,
            message: object.message,
            hint: object.hint.filter(|h| !h.is_empty()),
            retryable: object.retryable,
            request_id: object.request_id,
            details: object.details,
            status: None,
            retry_after: None,
            origin,
            source: None,
        }))
    }

    /// Adds a hint.
    #[must_use]
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.0.hint = Some(hint.into());
        self
    }

    /// Adds structured details.
    #[must_use]
    pub fn with_details(mut self, details: Value) -> Self {
        self.0.details = Some(details);
        self
    }

    /// Completes an input error (`invalid_input`, `invalid_json`) so agents
    /// can route on it: `details.field` names the offending field and a hint
    /// says what to do. Values already set are kept; other codes pass
    /// through unchanged.
    ///
    /// The field is the first field path quoted in the message
    /// (`` `ask.options[1].label` ``), else a flag it starts with (`--lang`),
    /// else a bare quoted key qualified with `scope` (`ask.step`), else
    /// `scope` itself. A `scope` that is a flag (`--lang`) always wins.
    #[must_use]
    pub fn with_input_context(self, scope: &str, hint: &str) -> Self {
        let field = infer_field(&self.0.message, scope);
        self.with_input_field(&field, hint)
    }

    /// [`Error::with_input_context`] with the field given exactly.
    #[must_use]
    pub fn with_input_field(mut self, field: &str, hint: &str) -> Self {
        if !matches!(
            self.0.code,
            ErrorCode::InvalidInput | ErrorCode::InvalidJson
        ) {
            return self;
        }
        let has_field = self
            .0
            .details
            .as_ref()
            .and_then(|d| d.get("field"))
            .is_some_and(|f| !f.is_null());
        if !has_field {
            match self.0.details.as_mut() {
                Some(Value::Object(m)) => {
                    m.insert("field".to_owned(), Value::String(field.to_owned()));
                }
                _ => self.0.details = Some(json!({ "field": field })),
            }
        }
        if self.0.hint.as_deref().is_none_or(str::is_empty) && !hint.is_empty() {
            self.0.hint = Some(hint.to_owned());
        }
        self
    }

    /// Overrides retryability.
    #[must_use]
    pub fn with_retryable(mut self, retryable: bool) -> Self {
        self.0.retryable = retryable;
        self
    }

    /// Records the server request ID.
    #[must_use]
    pub fn with_request_id(mut self, request_id: Option<String>) -> Self {
        self.0.request_id = request_id;
        self
    }

    /// Records the HTTP status.
    #[must_use]
    pub fn with_status(mut self, status: u16) -> Self {
        self.0.status = Some(status);
        self
    }

    /// Records a `Retry-After` delay.
    #[must_use]
    pub fn with_retry_after(mut self, retry_after: Option<Duration>) -> Self {
        self.0.retry_after = retry_after;
        self
    }

    /// Records where the error was produced.
    #[must_use]
    pub fn with_origin(mut self, origin: Origin) -> Self {
        self.0.origin = origin;
        self
    }

    /// Attaches the underlying cause (never rendered on the wire).
    #[must_use]
    pub fn with_source(mut self, source: impl StdError + Send + Sync + 'static) -> Self {
        self.0.source = Some(Box::new(source));
        self
    }

    /// The machine code.
    #[must_use]
    pub fn code(&self) -> &ErrorCode {
        &self.0.code
    }

    /// The message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.0.message
    }

    /// The hint, if any.
    #[must_use]
    pub fn hint(&self) -> Option<&str> {
        self.0.hint.as_deref()
    }

    /// Whether retrying the same request can succeed.
    #[must_use]
    pub fn retryable(&self) -> bool {
        self.0.retryable
    }

    /// The server request ID, if any.
    #[must_use]
    pub fn request_id(&self) -> Option<&str> {
        self.0.request_id.as_deref()
    }

    /// Structured details, if any.
    #[must_use]
    pub fn details(&self) -> Option<&Value> {
        self.0.details.as_ref()
    }

    /// The HTTP status, when the error came from peek-server.
    #[must_use]
    pub fn status(&self) -> Option<u16> {
        self.0.status
    }

    /// The server's `Retry-After`, if it sent one.
    #[must_use]
    pub fn retry_after(&self) -> Option<Duration> {
        self.0.retry_after
    }

    /// Where the error was produced.
    #[must_use]
    pub fn origin(&self) -> Origin {
        self.0.origin
    }

    /// Whether the request may never have reached its destination.
    #[must_use]
    pub fn is_transport(&self) -> bool {
        self.0.origin == Origin::Transport
    }

    /// The CLI exit code. Unknown codes fall back to the HTTP status class.
    #[must_use]
    pub fn exit_code(&self) -> ExitCode {
        match (&self.0.code, self.0.status) {
            (ErrorCode::Other(_), Some(status)) => ExitCode::from_http_status(status),
            (ErrorCode::Other(_), None) if self.0.retryable => ExitCode::Unavailable,
            (code, _) => code.exit_code(),
        }
    }

    /// The wire object.
    #[must_use]
    pub fn to_object(&self) -> ErrorObject {
        let i = &self.0;
        ErrorObject {
            code: i.code.clone(),
            message: i.message.clone(),
            hint: i.hint.clone(),
            retryable: i.retryable,
            request_id: i.request_id.clone(),
            details: i.details.clone(),
        }
    }

    /// `{"error":{…}}`, exactly what `--json` prints on stderr.
    #[must_use]
    pub fn envelope(&self) -> Value {
        json!({ "error": self.to_object() })
    }
}

/// See [`Error::with_input_context`].
fn infer_field(message: &str, scope: &str) -> String {
    if scope.starts_with("--") {
        return scope.to_owned();
    }
    let is_path = |t: &str| {
        !t.is_empty()
            && t.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '[' | ']' | '-'))
    };
    let quoted: Vec<&str> = message
        .split('`')
        .skip(1)
        .step_by(2)
        .filter(|t| is_path(t))
        .collect();
    if let Some(t) = quoted
        .iter()
        .find(|t| t.contains('.') || t.contains('[') || t.starts_with("--"))
    {
        return (*t).to_owned();
    }
    if scope.is_empty() {
        let flag = message
            .split_whitespace()
            .map(|w| w.trim_end_matches([':', ',', ';', '.']))
            .find(|w| w.starts_with("--") && w.len() > 2 && is_path(w));
        if let Some(f) = flag {
            return f.to_owned();
        }
        let env = message.split_whitespace().find(|w| {
            w.len() > 2
                && w.chars().all(|c| c.is_ascii_uppercase() || c == '_')
                && w.chars().any(|c| c.is_ascii_uppercase())
        });
        if let Some(e) = env {
            return e.to_owned();
        }
        return quoted
            .first()
            .map_or_else(|| "argument".to_owned(), |t| (*t).to_owned());
    }
    match quoted.first() {
        Some(t) if t.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') => {
            format!("{scope}.{t}")
        }
        _ => scope.to_owned(),
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.0.message, self.0.code)
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.0
            .source
            .as_deref()
            .map(|e| e as &(dyn StdError + 'static))
    }
}

impl From<ErrorObject> for Error {
    fn from(object: ErrorObject) -> Self {
        Self::from_object(object, Origin::Local)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blueprint_exit_code_table() {
        let table: &[(&str, u8)] = &[
            ("internal_error", 1),
            ("store_corrupt", 1),
            ("invalid_input", 2),
            ("invalid_json", 2),
            ("conflicting_flags", 2),
            ("nothing_to_send", 2),
            ("text_too_long", 2),
            ("caption_too_long", 2),
            ("question_too_long", 2),
            ("speak_too_long", 2),
            ("too_many_elements", 2),
            ("too_many_options", 2),
            ("image_unreadable", 2),
            ("image_too_large", 2),
            ("image_unsupported", 2),
            ("drawing_too_large", 2),
            ("unknown_config_key", 2),
            ("invalid_silicon_home", 2),
            ("slt_is_public_id", 2),
            ("not_logged_in", 3),
            ("session_rejected", 3),
            ("slt_rejected", 3),
            ("login_attempt_expired", 3),
            ("reconsent_required", 3),
            ("testing_secret_invalid", 3),
            ("testing_generation_changed", 3),
            ("side_not_registered", 4),
            ("drawing_not_registered", 4),
            ("side_taken", 4),
            ("drawing_invalid", 4),
            ("slot_busy", 4),
            ("queue_full", 4),
            ("send_not_found", 4),
            ("schedule_not_found", 4),
            ("schedule_full", 4),
            ("ask_not_found", 4),
            ("platform_unsupported", 4),
            ("cli_outdated", 4),
            ("store_schema_newer", 4),
            ("private_application_organization_required", 4),
            ("recipient_not_registered", 4),
            ("not_org_admin", 4),
            ("backend_unavailable", 5),
            ("iam_unavailable", 5),
            ("iam_misconfigured", 5),
            ("daemon_unavailable", 5),
            ("peek_service_unavailable", 5),
            ("no_gui_session", 5),
            ("app_update_pending", 5),
            ("speech_unavailable", 5),
            ("ting_unavailable", 5),
        ];
        for (name, exit) in table {
            let code = ErrorCode::parse(name);
            assert!(
                !matches!(code, ErrorCode::Other(_)),
                "{name} must be a known code"
            );
            assert_eq!(code.as_str(), *name);
            assert_eq!(code.exit_code().code(), *exit, "{name}");
        }
    }

    #[test]
    fn every_known_code_round_trips() {
        for code in ErrorCode::ALL {
            assert_eq!(&ErrorCode::parse(code.as_str()), code);
        }
    }

    #[test]
    fn unknown_codes_are_preserved_and_classified_by_status() {
        let code = ErrorCode::parse("invalid_grant");
        assert_eq!(code, ErrorCode::Other("invalid_grant".into()));
        let e = Error::new(code, "x").with_status(401);
        assert_eq!(e.exit_code(), ExitCode::NotAuthenticated);
        let e = Error::new(ErrorCode::parse("brand_new"), "x").with_status(503);
        assert_eq!(e.exit_code(), ExitCode::Unavailable);
        let e = Error::new(ErrorCode::parse("brand_new"), "x");
        assert_eq!(e.exit_code(), ExitCode::Internal);
    }

    #[test]
    fn envelope_has_the_blueprint_shape() -> Result<(), serde_json::Error> {
        let e = Error::new(ErrorCode::SideTaken, "position 3 is held by si:dj")
            .with_hint("choose a free one")
            .with_details(json!({"owner":"si:dj","free":[1,4]}));
        let v = e.envelope();
        assert_eq!(v["error"]["code"], "side_taken");
        assert_eq!(v["error"]["retryable"], false);
        assert!(v["error"]["request_id"].is_null());
        assert_eq!(v["error"]["details"]["free"][1], 4);
        let back: ErrorObject = serde_json::from_value(v["error"].clone())?;
        assert_eq!(back, e.to_object());
        Ok(())
    }

    #[test]
    fn lenient_decoding_of_minimal_objects() -> Result<(), serde_json::Error> {
        let o: ErrorObject =
            serde_json::from_str(r#"{"code":"slot_busy","message":"m","extra":1}"#)?;
        assert_eq!(o.code, ErrorCode::SlotBusy);
        assert!(!o.retryable);
        assert!(o.hint.is_none());
        Ok(())
    }

    #[test]
    fn queue_codes_are_refusals() {
        assert!(ErrorCode::QueueFull.default_retryable());
        assert!(!ErrorCode::SendNotFound.default_retryable());
        assert!(!ErrorCode::ScheduleNotFound.default_retryable());
        assert!(!ErrorCode::ScheduleFull.default_retryable());
        for c in [
            ErrorCode::QueueFull,
            ErrorCode::SendNotFound,
            ErrorCode::ScheduleNotFound,
            ErrorCode::ScheduleFull,
            ErrorCode::SlotBusy,
        ] {
            assert_eq!(c.exit_code(), ExitCode::Refused, "{c}");
        }
    }
}
