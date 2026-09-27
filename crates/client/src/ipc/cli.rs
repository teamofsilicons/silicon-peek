//! CLI ↔ peekd ops (BLUEPRINT §1.6 "CLI ops"), plus the `hello` handshake
//! both roles use and the `ask.result` event a `send --wait` connection
//! receives.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Empty, EventBody, Op};
use crate::{
    api::TelemetryEvent,
    error::{Error, ErrorCode, Result},
    identity::{ActorId, OrgId, SlotIndex, SlotInfo},
    ids::{AskId, ScheduleId, SendId},
    ipc::frame::blob_limit,
    schema::{
        ImageFormat, ImageHop, ImageRef,
        ask::{Answer, Ask, AskType},
        limits,
        send::{
            Notify, SendFlags, check_duration, check_expires_in, check_flags, check_isi,
            check_speak, check_tz_name, check_voice, normalize_language,
        },
        show::Show,
    },
    timestamp::Timestamp,
    ting::AnswerVia,
};

/// A non-fatal note attached to a result, e.g. `speak_language_unsupported`
/// or `drawing_fallback_active` (with the drawing's message and stack in
/// `details`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Warning {
    /// Stable code.
    pub code: String,
    /// What happened.
    pub message: String,
    /// Structured context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

/// Warning codes peek emits.
pub mod warnings {
    /// No TTS voice for the detected language; the text was shown as a pill.
    pub const SPEAK_LANGUAGE_UNSUPPORTED: &str = "speak_language_unsupported";
    /// Deepgram is not configured or unavailable; the text was shown as a pill.
    pub const SPEECH_UNAVAILABLE: &str = "speech_unavailable";
    /// TTS failed before any audio played.
    pub const SPEECH_FAILED: &str = "speech_failed";
    /// The drawing crashed and the fallback visual is showing.
    pub const DRAWING_FALLBACK_ACTIVE: &str = "drawing_fallback_active";
    /// The drawing's glass outline changes in more than 10% of test frames.
    pub const GLASS_OUTLINE_CHURN: &str = "glass_outline_churn";
    /// The Carbon's screen is locked or asleep: the send waits in the queue
    /// and is shown (and spoken) when the Carbon is back.
    pub const CARBON_AWAY: &str = "carbon_away";
    /// The Silicon is not an active Ting recipient: answers and notifications
    /// cannot be delivered until `peek ting enroll`.
    pub const TING_NOT_ENROLLED: &str = "ting_not_enrolled";
    /// `$ISI` was invalid and was left out of the send.
    pub const ISI_IGNORED: &str = "isi_ignored";
    /// Peek.app is paused by the Carbon: the send waits and is shown when
    /// they resume.
    pub const CARBON_PAUSED: &str = "carbon_paused";
    /// The Mac's time zone could not be read; `--at`/`--expires-at` values
    /// without an offset were read as UTC (CLI-local).
    pub const TIMEZONE_FALLBACK_UTC: &str = "timezone_fallback_utc";
}

/// The bundled Peek.app a CLI copy carries (from its package's `Peek.app.info`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundledApp {
    /// `CFBundleVersion` (`major*1_000_000 + minor*1_000 + patch`).
    pub build: u64,
    /// `CFBundleShortVersionString`.
    pub short_version: String,
    /// Absolute path of `Peek.app.zip`.
    pub zip_path: String,
    /// Hex SHA-256 of the zip.
    pub zip_sha256: String,
}

/// The CLI's handshake fields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CliHello {
    /// The CLI version.
    pub cli_version: String,
    /// Protocol majors the CLI speaks.
    pub protocols: Vec<u32>,
    /// `macos-aarch64` or `macos-x86_64`.
    pub platform: String,
    /// The app bundled with this CLI copy, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundled_app: Option<BundledApp>,
}

/// Peek.app's handshake fields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiHello {
    /// `CFBundleVersion`.
    pub app_build: u64,
    /// `CFBundleShortVersionString`.
    pub app_version: String,
    /// Protocol majors the app speaks.
    pub protocols: Vec<u32>,
}

/// `hello`: the first frame on every connection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Hello {
    /// A CLI connection (one request, one reply; or `send --wait`).
    Cli(CliHello),
    /// Peek.app's long-lived connection.
    Ui(UiHello),
}

impl Hello {
    /// The CLI hello for this build.
    #[must_use]
    pub fn cli(platform: impl Into<String>, bundled_app: Option<BundledApp>) -> Self {
        Self::Cli(CliHello {
            cli_version: crate::VERSION.to_owned(),
            protocols: super::SUPPORTED_PROTOCOLS.to_vec(),
            platform: platform.into(),
            bundled_app,
        })
    }

    /// Protocols the peer offered.
    #[must_use]
    pub fn protocols(&self) -> &[u32] {
        match self {
            Self::Cli(h) => &h.protocols,
            Self::Ui(h) => &h.protocols,
        }
    }
}

/// App state reported in the handshake.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloApp {
    /// The installed Peek.app build.
    pub build: u64,
    /// Whether Peek.app is connected.
    pub ui_running: bool,
}

/// `hello` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloResult {
    /// The negotiated protocol.
    pub protocol: u32,
    /// The daemon version.
    pub peekd_version: String,
    /// App state (sent to CLIs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<HelloApp>,
    /// Capabilities of this peekd (additive; absent from peekd ≤ 0.1.1,
    /// which reads as none). See [`features`].
    #[serde(default)]
    pub features: Vec<String>,
}

impl HelloResult {
    /// Whether peekd announced `feature`.
    #[must_use]
    pub fn has_feature(&self, feature: &str) -> bool {
        self.features.iter().any(|f| f == feature)
    }
}

/// peekd capabilities announced in [`HelloResult::features`]. The protocol
/// stays 1; a CLI refuses a new flag or command against a peekd that lacks
/// its feature instead of letting an older peekd silently ignore it.
pub mod features {
    /// Always-queue FIFO, `queue.list`, `queue.clear`, `send.cancel`, `queue_full`.
    pub const QUEUE_V2: &str = "queue_v2";
    /// `expires_in_s` / `expires_at` on every send kind.
    pub const EXPIRY_ALL: &str = "expiry_all";
    /// `SendOp::replace`.
    pub const REPLACE: &str = "replace";
    /// `SendOp::due_at`, `schedule.list`, `schedule.cancel`, `schedule.clear`.
    pub const SCHEDULE: &str = "schedule";
    /// `Notify::Shown` and `peek.send.shown`.
    pub const NOTIFY_SHOWN: &str = "notify_shown";
    /// Every feature of peekd 0.1.2.
    pub const ALL: [&str; 5] = [QUEUE_V2, EXPIRY_ALL, REPLACE, SCHEDULE, NOTIFY_SHOWN];
}

impl Op for Hello {
    const NAME: &'static str = "hello";
    type Output = HelloResult;
}

/// `attach`: record this home with peekd.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attach {}

/// `attach` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachResult {
    /// peekd's stable ID for the home.
    pub home_id: String,
    /// The verified actor.
    pub actor_id: ActorId,
    /// The verified org.
    pub org_id: OrgId,
}

impl Op for Attach {
    const NAME: &'static str = "attach";
    type Output = AttachResult;
}

/// Why a home detaches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetachReason {
    /// `peek logout`.
    Logout,
}

/// `detach`: cancel this actor's undelivered outbox rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Detach {
    /// Why.
    pub reason: DetachReason,
}

/// `detach` result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetachResult {
    /// Rows cancelled.
    pub cancelled_rows: u64,
}

impl Op for Detach {
    const NAME: &'static str = "detach";
    type Output = DetachResult;
}

/// `status`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusOp {}

/// The Silicon's active drawing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DrawingStatus {
    /// Hex SHA-256.
    pub sha256: String,
    /// Size.
    pub bytes: u64,
    /// Whether it is live (false while the fallback visual shows).
    pub active: bool,
    /// The last runtime error, if any.
    #[serde(default)]
    pub last_error: Option<String>,
    /// The backend copy: `synced`, `pending` or `authority_required`
    /// (additive; kept apart from the Ting `deliveries`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_sync: Option<String>,
}

/// Whether the Carbon can see bubbles right now (Peek.app's `presence`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CarbonStatus {
    /// False while the screen is locked or asleep: sends wait in the queue.
    pub available: bool,
    /// Why.
    pub reason: crate::ipc::ui::PresenceReason,
    /// The Carbon paused all peeks in Peek.app (additive).
    #[serde(default)]
    pub paused: bool,
}

impl Default for CarbonStatus {
    fn default() -> Self {
        Self {
            available: true,
            reason: crate::ipc::ui::PresenceReason::Ok,
            paused: false,
        }
    }
}

/// Why a Silicon's current send is not on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeldReason {
    /// The Carbon's screen is locked or asleep.
    CarbonAway,
    /// The Carbon paused Peek.
    Paused,
    /// Peek.app is not connected, or an app update swap is in progress.
    AppNotRunning,
}

const fn five() -> u32 {
    5
}

/// The Silicon's queue.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueStatus {
    /// Sends waiting (not on screen): waiting plus due scheduled sends
    /// waiting for room. The name is kept for older CLIs.
    pub pending: u32,
    /// The current send (on screen or held).
    #[serde(default)]
    pub on_screen: Option<SendId>,
    /// The same number as `pending`.
    #[serde(default)]
    pub waiting: u32,
    /// How many may wait (5).
    #[serde(default = "five")]
    pub limit: u32,
    /// Scheduled sends that are not due yet.
    #[serde(default)]
    pub scheduled: u32,
    /// Why the current send is not on screen, when it is held.
    #[serde(default)]
    pub held: Option<HeldReason>,
}

impl Default for QueueStatus {
    fn default() -> Self {
        Self {
            pending: 0,
            on_screen: None,
            waiting: 0,
            limit: five(),
            scheduled: 0,
            held: None,
        }
    }
}

/// The Silicon's Ting deliveries (the drawing's backend copy is
/// [`DrawingStatus::server_sync`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveriesStatus {
    /// Rows still being delivered.
    pub pending: u32,
    /// Rows waiting for a fresh login or re-consent.
    pub authority_required: u32,
    /// The last delivery error code, if any.
    #[serde(default)]
    pub last_error: Option<String>,
}

/// `status` result.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StatusResult {
    /// The registered position.
    pub slot: Option<SlotInfo>,
    /// The registered drawing.
    pub drawing: Option<DrawingStatus>,
    /// The send queue.
    pub queue: QueueStatus,
    /// Asks waiting for the Carbon.
    pub pending_asks: u32,
    /// Delivery backlog.
    pub deliveries: DeliveriesStatus,
    /// Whether Peek.app is connected.
    pub ui_running: bool,
    /// Whether the Carbon can see bubbles (additive; absent from older peekd).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub carbon: Option<CarbonStatus>,
    /// Pending notes (e.g. `drawing_fallback_active`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<Warning>,
}

impl Op for StatusOp {
    const NAME: &'static str = "status";
    type Output = StatusResult;
}

/// `register.side`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterSide {
    /// 1..=8.
    pub index: SlotIndex,
}

/// `register.side` result.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegisterSideResult {
    /// The position now held.
    pub slot: SlotInfo,
    /// The position left, when this was a move.
    pub moved_from: Option<SlotIndex>,
    /// The Carbon hotkey for it, e.g. `ctrl+cmd+5`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hotkey: Option<String>,
    /// Pending notes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<Warning>,
}

impl Op for RegisterSide {
    const NAME: &'static str = "register.side";
    type Output = RegisterSideResult;
}

/// `register.drawing`: one blob, the script (≤ 256 KiB).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterDrawing {
    /// The file name, for messages.
    pub filename: String,
    /// Validate only (`--check`).
    pub check_only: bool,
    /// Return a PNG preview blob (`--preview`).
    pub preview: bool,
    /// Return the op stream of this test frame (`--dump-frame`).
    pub dump_frame: Option<u32>,
}

/// Validation statistics over the test frames.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DrawingStats {
    /// Frames rendered.
    pub frames: u32,
    /// Median frame time.
    pub p50_ms: f64,
    /// 95th percentile frame time.
    pub p95_ms: f64,
    /// Slowest frame.
    pub max_ms: f64,
    /// Most draw ops in one frame.
    pub ops_max: u32,
    /// Glass outline rebuilds.
    pub glass_rebuilds: u32,
}

/// Whether a drawing copy is queued for peek-server.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServerSync {
    /// A `drawing.put` outbox row is queued.
    Pending,
    /// Not uploaded (`--check`, or unchanged).
    Skipped,
}

/// `register.drawing` result (plus a PNG blob with `preview`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegisterDrawingResult {
    /// Hex SHA-256 of the script.
    pub sha256: String,
    /// Script size.
    pub bytes: u64,
    /// Test-frame statistics.
    pub stats: DrawingStats,
    /// Validation warnings.
    #[serde(default)]
    pub warnings: Vec<Warning>,
    /// `peek.log` output from the test frames.
    #[serde(default)]
    pub logs: Vec<Value>,
    /// Whether it is now the active drawing.
    pub active: bool,
    /// The Silicon's position, if registered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<SlotIndex>,
    /// Server copy state.
    pub server_sync: ServerSync,
    /// The dumped frame, with `dump_frame`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dump: Option<Value>,
}

impl Op for RegisterDrawing {
    const NAME: &'static str = "register.drawing";
    type Output = RegisterDrawingResult;
}

/// `unregister`: release the position, cancel asks, delete the drawing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unregister {}

/// `unregister` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnregisterResult {
    /// The position released.
    pub released_slot: Option<SlotIndex>,
    /// Asks cancelled (no tings are sent for them).
    pub cancelled_asks: Vec<AskId>,
    /// Non-ask sends that were current or waiting (additive).
    #[serde(default)]
    pub cancelled_sends: Vec<SendId>,
    /// Scheduled sends cancelled before they were due (additive).
    #[serde(default)]
    pub cancelled_scheduled: Vec<ScheduleId>,
}

impl Op for Unregister {
    const NAME: &'static str = "unregister";
    type Output = UnregisterResult;
}

/// `send`: images travel as blobs, referenced as `{"blob":k}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SendOp {
    /// `$ISI`, when set.
    pub isi: Option<String>,
    /// `--speak`.
    pub speak: Option<String>,
    /// `--show`.
    pub show: Option<Show>,
    /// `--ask`.
    pub ask: Option<Ask>,
    /// `--voice` (or config `voice`).
    pub voice: Option<String>,
    /// `--lang` (or config `language`).
    pub lang: Option<String>,
    /// `--notify` (or config `notify`).
    #[serde(default)]
    pub notify: Vec<Notify>,
    /// `--duration`, in milliseconds.
    pub duration_ms: Option<u64>,
    /// `--expires-in`, seconds from peekd's receipt. Any kind (0.1.1: asks
    /// only). Not with `due_at`.
    pub expires_in_s: Option<u64>,
    /// `--wait`: keep the connection open for `ask.result`.
    #[serde(default)]
    pub wait: bool,
    /// `--expires-at`: an absolute deadline. Not with `expires_in_s`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Timestamp>,
    /// `--in` / `--at`: schedule instead of queueing now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_at: Option<Timestamp>,
    /// The IANA zone the CLI used to read `--at`/`--expires-at` (display
    /// only; at most 64 characters of `[A-Za-z0-9_+-/]`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tz: Option<String>,
    /// `--replace`: take over the Silicon's current bubble instead of queueing.
    #[serde(default, skip_serializing_if = "is_false")]
    pub replace: bool,
}

impl SendOp {
    /// [`SendOp::validate_at`] with peekd's tolerance for CLI→peekd latency
    /// ([`limits::SCHEDULE_SLACK_MS`]) at the current time.
    ///
    /// # Errors
    /// The first violated rule's error.
    pub fn validate(&self, blobs: &[Vec<u8>]) -> Result<()> {
        self.validate_at(blobs, Timestamp::now(), limits::SCHEDULE_SLACK_MS)
    }

    /// Every §7.4 rule, plus blob consistency (each blob is referenced exactly
    /// once, is at most 10 MiB and is a supported image), then the 0.1.2
    /// deadline rules against `now`: `--tz`, `due_at` within `(now − slack,
    /// now + 365 d + slack]`, and `expires_at` 10 s – 7 d after `now` (slack on
    /// both ends) or, for a scheduled send, 10 s – 7 d after `due_at` (no
    /// slack). The CLI passes `slack_ms = 0`; peekd passes
    /// [`limits::SCHEDULE_SLACK_MS`].
    ///
    /// # Errors
    /// The first violated rule's error.
    pub fn validate_at(&self, blobs: &[Vec<u8>], now: Timestamp, slack_ms: i64) -> Result<()> {
        check_flags(SendFlags {
            speak: self.speak.is_some(),
            show: self.show.is_some(),
            ask: self.ask.is_some(),
            voice: self.voice.is_some(),
            lang: self.lang.is_some(),
            duration: self.duration_ms.is_some(),
            expires_in: self.expires_in_s.is_some(),
            wait: self.wait,
            expires_at: self.expires_at.is_some(),
            schedule_in: false,
            schedule_at: self.due_at.is_some(),
            tz: self.tz.is_some(),
            replace: self.replace,
        })?;
        if let Some(s) = &self.speak {
            check_speak(s)?;
        }
        if let Some(isi) = &self.isi {
            check_isi(isi)?;
        }
        if let Some(v) = &self.voice {
            check_voice(v)?;
        }
        if let Some(l) = &self.lang {
            normalize_language(l)?;
        }
        if let Some(ms) = self.duration_ms {
            if ms % 1000 != 0 {
                return Err(Error::invalid_input(format!(
                    "duration_ms {ms} must be a whole number of seconds"
                )));
            }
            check_duration(ms / 1000)?;
        }
        let hop = ImageHop::Ipc { blobs: blobs.len() };
        let mut referenced = vec![0u32; blobs.len()];
        let mut count = |r: &ImageRef| {
            if let ImageRef::Blob { blob } = r
                && let Some(c) = referenced.get_mut(*blob)
            {
                *c += 1;
            }
        };
        if let Some(show) = &self.show {
            show.validate(hop)?;
            show.images().for_each(&mut count);
        }
        if let Some(ask) = &self.ask {
            ask.validate(hop)?;
            ask.images().for_each(&mut count);
        }
        let per_blob = blob_limit(<Self as Op>::NAME).unwrap_or(limits::IMAGE_MAX_BYTES);
        for (k, (n, bytes)) in referenced.iter().zip(blobs).enumerate() {
            if *n != 1 {
                return Err(Error::invalid_input(format!(
                    "blob {k} is referenced {n} times; every image blob is referenced exactly once"
                )));
            }
            if bytes.len() > per_blob {
                return Err(Error::new(
                    ErrorCode::ImageTooLarge,
                    format!(
                        "image blob {k} is {} bytes; the limit is {per_blob}",
                        bytes.len()
                    ),
                ));
            }
            if ImageFormat::sniff(bytes).is_none() {
                return Err(Error::new(
                    ErrorCode::ImageUnsupported,
                    format!("image blob {k} is not PNG, JPEG, HEIC, WebP or GIF"),
                ));
            }
        }
        if let Some(tz) = &self.tz {
            check_tz_name(tz)?;
        }
        self.check_deadlines(now, slack_ms)?;
        if let Some(s) = self.expires_in_s {
            check_expires_in(s)?;
        }
        Ok(())
    }

    fn check_deadlines(&self, now: Timestamp, slack_ms: i64) -> Result<()> {
        const SECOND_MS: i64 = 1000;
        let secs_ms = |s: u64| {
            i64::try_from(s)
                .unwrap_or(i64::MAX)
                .saturating_mul(SECOND_MS)
        };
        let horizon = secs_ms(limits::SCHEDULE_IN_MAX_S);
        let expires_min = secs_ms(limits::EXPIRES_IN_MIN_S);
        let expires_max = secs_ms(limits::EXPIRES_IN_MAX_S);
        let now_ms = now.unix_ms();
        if let Some(due) = self.due_at {
            let due_ms = due.unix_ms();
            if due_ms <= now_ms.saturating_sub(slack_ms) {
                return Err(Error::invalid_input(format!(
                    "`--at` {due} is in the past (now {now})"
                ))
                .with_hint("give a time in the future, or use --in <DURATION>")
                .with_details(json!({"field": "--at", "value": due, "now": now})));
            }
            if due_ms > now_ms.saturating_add(horizon).saturating_add(slack_ms) {
                return Err(Error::invalid_input(format!(
                    "`--at` {due} is more than 365 days ahead (now {now})"
                ))
                .with_hint("schedule at most 365 days ahead")
                .with_details(json!({"field": "--at", "value": due, "now": now})));
            }
        }
        if let Some(exp) = self.expires_at {
            let exp_ms = exp.unix_ms();
            let (low, high, base, what) = match self.due_at {
                Some(due) => (
                    due.unix_ms().saturating_add(expires_min),
                    due.unix_ms().saturating_add(expires_max),
                    due,
                    "the due time",
                ),
                None => (
                    now_ms.saturating_add(expires_min).saturating_sub(slack_ms),
                    now_ms.saturating_add(expires_max).saturating_add(slack_ms),
                    now,
                    "now",
                ),
            };
            if exp_ms < low || exp_ms > high {
                return Err(Error::invalid_input(format!(
                    "`--expires-at` {exp} must be 10 s – 7 d after {what} ({base})"
                ))
                .with_hint(if self.due_at.is_some() {
                    "--expires-at <DATETIME> between 10 s and 7 days after --at/--in"
                } else {
                    "--expires-at <DATETIME> between 10 s and 7 days from now, or --expires-in 15m"
                })
                .with_details(json!({
                    "field": "--expires-at",
                    "value": exp,
                    "min": Timestamp::from_unix_ms(low),
                    "max": Timestamp::from_unix_ms(high)
                })));
            }
        }
        Ok(())
    }

    /// The send's kind: `speak`, `show`, `ask`, `speak+show` or `speak+ask`.
    #[must_use]
    pub fn kind(&self) -> String {
        let mut parts = Vec::new();
        if self.speak.is_some() {
            parts.push("speak");
        }
        if self.show.is_some() {
            parts.push("show");
        }
        if self.ask.is_some() {
            parts.push("ask");
        }
        parts.join("+")
    }
}

/// Whether a send is on screen, waiting or scheduled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SendStatus {
    /// It became the Silicon's current bubble and is sliding in.
    Showing,
    /// Waiting in the Silicon's queue, or current but held (Carbon away or
    /// paused, Peek.app not running).
    Queued,
    /// `--in`/`--at`: stored until due.
    Scheduled,
}

/// TTS status of a send.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeechStatus {
    /// Streaming from Deepgram.
    Pending,
    /// Served from the local TTS cache.
    Cached,
    /// No speech (no `--speak`, or speech is unavailable).
    Skipped,
    /// The detected language has no Aura-2 voice.
    UnsupportedLanguage,
}

/// TTS details of a send.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeechInfo {
    /// Status.
    pub status: SpeechStatus,
    /// The Aura-2 voice.
    #[serde(default)]
    pub model: Option<String>,
    /// Characters spoken.
    pub chars: u32,
}

/// `send` result. The 0.1.2 fields are always serialized (null when they do
/// not apply) so `--json` output has one stable shape; they default when a
/// 0.1.1 peekd leaves them out.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SendResult {
    /// The send.
    pub send_id: SendId,
    /// The ask, with `--ask`.
    pub ask_id: Option<AskId>,
    /// The slot.
    pub slot: SlotIndex,
    /// Showing, queued or scheduled.
    pub status: SendStatus,
    /// TTS details, with `--speak`.
    #[serde(default)]
    pub speech: Option<SpeechInfo>,
    /// Notes.
    #[serde(default)]
    pub warnings: Vec<Warning>,
    /// 0 = current (showing or held), 1..=5 waiting (1 = next); null for
    /// scheduled sends.
    #[serde(default)]
    pub queue_position: Option<u32>,
    /// Sends waiting behind the current one after this send (waiting plus
    /// due-waiting); null for scheduled sends.
    #[serde(default)]
    pub waiting: Option<u32>,
    /// When it expires.
    #[serde(default)]
    pub expires_at: Option<Timestamp>,
    /// The scheduled send, with `--in`/`--at`.
    #[serde(default)]
    pub schedule_id: Option<ScheduleId>,
    /// When it is due, with `--in`/`--at`.
    #[serde(default)]
    pub due_at: Option<Timestamp>,
    /// The IANA zone `--at` was read in (echo of the op's `tz`).
    #[serde(default)]
    pub tz: Option<String>,
    /// `--replace`: the send it took over, if one was current.
    #[serde(default)]
    pub replaced_send_id: Option<SendId>,
}

impl Op for SendOp {
    const NAME: &'static str = "send";
    type Output = SendResult;
}

/// An ask's lifecycle state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AskState {
    /// On screen or queued.
    Pending,
    /// Answered.
    Answered,
    /// Closed without an answer.
    Dismissed,
    /// Ran past `--expires-in` / `--expires-at`.
    Expired,
    /// Cancelled by the Silicon (or `unregister`/`logout`).
    Cancelled,
    /// Taken over by the Silicon's own `--replace`; no ting.
    Replaced,
}

/// Delivery state of an answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    /// In the outbox.
    Pending,
    /// Waiting for a fresh login or re-consent.
    AuthorityRequired,
    /// Ting accepted it.
    Accepted,
    /// Ran past `delivery_max_age_hours`.
    Expired,
    /// Cancelled (logout).
    Cancelled,
    /// Refused permanently (a bug).
    Failed,
    /// Delivered on stdout to a live `send --wait` instead of a ting.
    Wait,
}

/// Delivery details.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryInfo {
    /// State.
    pub status: DeliveryState,
    /// Ting's message ID, once accepted.
    #[serde(default)]
    pub ting_id: Option<String>,
    /// Accepted but muted by the recipient.
    #[serde(default)]
    pub silent: Option<bool>,
}

/// One ask, as `ask.get` and `ask.list` report it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AskInfo {
    /// The ask.
    pub ask_id: AskId,
    /// Its state.
    pub state: AskState,
    /// The answer, once answered.
    #[serde(default)]
    pub answer: Option<Answer>,
    /// How it was answered.
    #[serde(default)]
    pub via: Option<AnswerVia>,
    /// When it was answered.
    #[serde(default)]
    pub answered_at: Option<Timestamp>,
    /// Delivery of the answer.
    #[serde(default)]
    pub delivery: Option<DeliveryInfo>,
    /// The send that carried it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_id: Option<SendId>,
    /// The question.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    /// The type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask_type: Option<AskType>,
    /// The final transcript of a voice answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript: Option<String>,
    /// When it was asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<Timestamp>,
    /// When it expires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Timestamp>,
}

/// `ask.get`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskGet {
    /// The ask.
    pub ask_id: AskId,
}

impl Op for AskGet {
    const NAME: &'static str = "ask.get";
    type Output = AskInfo;
}

/// `ask.list`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskList {
    /// Filter by state.
    pub state: Option<AskState>,
    /// At most this many (default chosen by peekd).
    pub limit: Option<u32>,
}

/// `ask.list` result.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AskListResult {
    /// Newest first.
    pub asks: Vec<AskInfo>,
}

impl Op for AskList {
    const NAME: &'static str = "ask.list";
    type Output = AskListResult;
}

/// `ask.cancel`: slide the ask away; no ting is sent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskCancel {
    /// The ask.
    pub ask_id: AskId,
}

/// `ask.cancel` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskCancelResult {
    /// The ask.
    pub ask_id: AskId,
    /// The ask's state after the call: `cancelled`, or the state it had
    /// already closed in (`answered`, `dismissed`, `expired`, `replaced`),
    /// when there was nothing left to cancel.
    pub state: AskState,
}

impl Op for AskCancel {
    const NAME: &'static str = "ask.cancel";
    type Output = AskCancelResult;
}

/// `history`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct History {
    /// At most this many (≤ 200).
    pub limit: Option<u32>,
    /// Items older than this send ID.
    pub before: Option<String>,
}

impl History {
    /// Checks `limit ≤ 200`.
    ///
    /// # Errors
    /// `invalid_input`.
    pub fn validate(&self) -> Result<()> {
        match self.limit {
            Some(0) => Err(Error::invalid_input("--limit must be at least 1")),
            Some(n) if n > limits::HISTORY_MAX_LIMIT => Err(Error::invalid_input(format!(
                "--limit {n} exceeds {}",
                limits::HISTORY_MAX_LIMIT
            ))),
            _ => Ok(()),
        }
    }
}

/// One history row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryItem {
    /// The send.
    pub send_id: SendId,
    /// What it carried, e.g. `speak`, `show`, `ask`, `speak+show`.
    pub kind: String,
    /// When it was sent.
    pub created_at: Timestamp,
    /// When its bubble closed.
    #[serde(default)]
    pub closed_at: Option<Timestamp>,
    /// Why it closed (`auto`, `speech_done`, `dismissed`, `replaced`, …).
    #[serde(default)]
    pub close_reason: Option<String>,
    /// Its ask.
    #[serde(default)]
    pub ask_id: Option<AskId>,
    /// Its ask's state.
    #[serde(default)]
    pub ask_state: Option<AskState>,
    /// The warnings the send returned (e.g. `carbon_away`,
    /// `speak_language_unsupported`), then what went wrong after it
    /// returned, e.g. `speech_failed` when its speech became a text pill.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<Warning>,
    /// When it appeared on screen (additive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shown_at: Option<Timestamp>,
    /// Its deadline (additive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Timestamp>,
    /// The scheduled send it came from (additive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule_id: Option<ScheduleId>,
    /// When it was due, for a scheduled send (additive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_at: Option<Timestamp>,
}

/// `history` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryResult {
    /// Newest first.
    pub items: Vec<HistoryItem>,
}

impl Op for History {
    const NAME: &'static str = "history";
    type Output = HistoryResult;
}

// ------------------------------------------------------------ queue ops

/// `queue.list`: this Silicon's current send and the ones waiting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueList {}

/// Where a send sits in its queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueItemState {
    /// The current send, on screen.
    OnScreen,
    /// The current send, held (Carbon away or paused, Peek.app not running).
    Held,
    /// Waiting (at most five).
    Waiting,
    /// A due scheduled send waiting for a free spot.
    DueWaiting,
}

/// One send of `queue.list`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueItem {
    /// The send.
    pub send_id: SendId,
    /// Its ask.
    pub ask_id: Option<AskId>,
    /// `speak`, `show`, `ask`, `speak+show` or `speak+ask`.
    pub kind: String,
    /// At most 60 characters (may be empty).
    pub summary: String,
    /// Where it sits.
    pub state: QueueItemState,
    /// 0 = current; 1.. = the order it will be shown in.
    pub queue_position: u32,
    /// When `peek send` ran.
    pub created_at: Timestamp,
    /// When it entered the queue (the fire time for a scheduled send).
    pub queued_at: Timestamp,
    /// Milliseconds since `created_at`.
    pub age_ms: u64,
    /// Its deadline.
    pub expires_at: Option<Timestamp>,
    /// When it appeared.
    pub shown_at: Option<Timestamp>,
    /// The scheduled send it came from.
    pub schedule_id: Option<ScheduleId>,
    /// When it was due.
    pub due_at: Option<Timestamp>,
}

/// `queue.list` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueListResult {
    /// The Silicon's position (null, with empty lists, when it holds none).
    pub slot: Option<SlotIndex>,
    /// How many may wait (5).
    pub limit: u32,
    /// The current send (state `on_screen` or `held`).
    pub on_screen: Option<QueueItem>,
    /// Waiting, then due-waiting, in show order.
    pub waiting: Vec<QueueItem>,
    /// Why the current send is held.
    pub held: Option<HeldReason>,
    /// Scheduled sends not due yet.
    pub scheduled: u32,
    /// How many may be scheduled (500).
    pub scheduled_limit: u32,
}

impl Op for QueueList {
    const NAME: &'static str = "queue.list";
    type Output = QueueListResult;
}

/// `queue.clear`: drop every waiting send (and due scheduled sends waiting
/// for room); with `all` also the current one. No ting is sent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueClear {
    /// Also withdraw the current send.
    #[serde(default)]
    pub all: bool,
}

/// `queue.clear` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueClearResult {
    /// Waiting and due-waiting sends (plus the current one with `all`), in
    /// queue order.
    pub cancelled: Vec<SendId>,
    /// The current send before the call.
    pub on_screen: Option<SendId>,
    /// Whether the current send was withdrawn.
    pub on_screen_cancelled: bool,
}

impl Op for QueueClear {
    const NAME: &'static str = "queue.clear";
    type Output = QueueClearResult;
}

/// `send.cancel`: withdraw one send (on screen, waiting or scheduled; any
/// kind). No ting is sent. The ID travels as `target` because `id` is the
/// request envelope's own field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendCancel {
    /// `snd_…`, `ask_…` or `sch_…`.
    pub target: String,
}

/// Where a cancelled send was.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelledFrom {
    /// The current send, on screen.
    OnScreen,
    /// The current send, held.
    Held,
    /// Waiting.
    Waiting,
    /// A due scheduled send waiting for room.
    DueWaiting,
    /// Scheduled, not due yet.
    Scheduled,
    /// It had already closed; nothing was cancelled.
    Closed,
}

/// `send.cancel` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendCancelResult {
    /// The send.
    pub send_id: SendId,
    /// Its ask.
    pub ask_id: Option<AskId>,
    /// Its scheduled send.
    pub schedule_id: Option<ScheduleId>,
    /// Where it was.
    pub was: CancelledFrom,
    /// Its place in the queue (for `on_screen`, `held`, `waiting`, `due_waiting`).
    pub queue_position: Option<u32>,
    /// `cancelled`, or for `was: closed` the final close reason or ask state
    /// it already had (`answered`, `dismissed`, `expired`, `replaced`,
    /// `auto`, `speech_done`, …).
    pub state: String,
    /// When it was due, for a scheduled send (additive).
    #[serde(default)]
    pub due_at: Option<Timestamp>,
    /// The IANA zone its `--at` was given in, for a scheduled send (additive).
    #[serde(default)]
    pub tz: Option<String>,
}

impl Op for SendCancel {
    const NAME: &'static str = "send.cancel";
    type Output = SendCancelResult;
}

/// `schedule.list`: scheduled sends that are not due yet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleList {}

/// One scheduled send.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduledItem {
    /// The scheduled send.
    pub schedule_id: ScheduleId,
    /// The send it becomes when it fires.
    pub send_id: SendId,
    /// Its ask.
    pub ask_id: Option<AskId>,
    /// `speak`, `show`, `ask`, `speak+show` or `speak+ask`.
    pub kind: String,
    /// At most 60 characters (may be empty).
    pub summary: String,
    /// When it is due.
    pub due_at: Timestamp,
    /// The IANA zone `--at` was given in.
    pub tz: Option<String>,
    /// Its deadline.
    pub expires_at: Option<Timestamp>,
    /// `--replace`.
    pub replace: bool,
    /// When `peek send` ran.
    pub created_at: Timestamp,
}

/// `schedule.list` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleListResult {
    /// Soonest first (then by schedule ID).
    pub scheduled: Vec<ScheduledItem>,
    /// How many may be scheduled (500).
    pub limit: u32,
    /// The Silicon's position (additive; null when it holds none).
    #[serde(default)]
    pub slot: Option<SlotIndex>,
}

impl Op for ScheduleList {
    const NAME: &'static str = "schedule.list";
    type Output = ScheduleListResult;
}

/// `schedule.cancel`: cancel one scheduled send before it is due. The ID
/// travels as `target` (`id` is the request envelope's own field).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleCancel {
    /// `sch_…` or its `snd_…`.
    pub target: String,
}

/// `schedule.cancel` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleCancelResult {
    /// The scheduled send.
    pub schedule_id: ScheduleId,
    /// Its send.
    pub send_id: SendId,
    /// `cancelled`, or `fired` when it already came due (then it is in the
    /// queue or closed; use `send.cancel`).
    pub state: String,
    /// When it was due (additive).
    #[serde(default)]
    pub due_at: Option<Timestamp>,
    /// The IANA zone its `--at` was given in (additive).
    #[serde(default)]
    pub tz: Option<String>,
}

impl Op for ScheduleCancel {
    const NAME: &'static str = "schedule.cancel";
    type Output = ScheduleCancelResult;
}

/// `schedule.clear`: cancel every scheduled send of this Silicon.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleClear {}

/// `schedule.clear` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleClearResult {
    /// The scheduled sends cancelled.
    pub cancelled: Vec<ScheduleId>,
}

impl Op for ScheduleClear {
    const NAME: &'static str = "schedule.clear";
    type Output = ScheduleClearResult;
}

/// The config subset peekd mirrors.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigSyncConfig {
    /// Default voice.
    pub voice: Option<String>,
    /// Default TTS language.
    pub language: Option<String>,
    /// Default notifications.
    pub notify: Vec<Notify>,
    /// This home's telemetry, as the CLI that synced it sees it: its
    /// config, and also its environment (`PEEK_TELEMETRY`,
    /// `SPACE_STATION_TELEMETRY`, `SILICON_TELEMETRY`).
    pub telemetry: bool,
    /// Additive: `telemetry` is `false` only because that CLI's environment
    /// opts out (the home's config allows telemetry). peekd lifts such an
    /// opt-out once a CLI of the home hands it telemetry again.
    #[serde(default, skip_serializing_if = "is_false")]
    pub env_opt_out: bool,
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde's skip_serializing_if signature
fn is_false(b: &bool) -> bool {
    !*b
}

/// `config.sync`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigSync {
    /// The mirrored config.
    pub config: ConfigSyncConfig,
}

impl Op for ConfigSync {
    const NAME: &'static str = "config.sync";
    type Output = Empty;
}

/// `telemetry` (CLI and UI): hand events to peekd's relay.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Telemetry {
    /// Web-SDK-shaped events.
    pub events: Vec<TelemetryEvent>,
}

impl Op for Telemetry {
    const NAME: &'static str = "telemetry";
    type Output = Empty;
}

/// `Peek.app.info`, the sidecar next to `Peek.app.zip`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppOfferInfo {
    /// `ai.tos.peek` (or `ai.tos.peek.dev`).
    pub bundle_id: String,
    /// `CFBundleVersion`, an integer that strictly increases per release.
    pub bundle_version: u64,
    /// `CFBundleShortVersionString`.
    pub short_version: String,
    /// Signing team (empty for dev builds).
    pub team_id: String,
    /// Hex SHA-256 of the zip.
    pub zip_sha256: String,
    /// `LSMinimumSystemVersion`.
    pub minimum_system_version: String,
}

impl AppOfferInfo {
    /// Parses the `key=value` sidecar.
    ///
    /// # Errors
    /// `invalid_input` naming the missing or malformed key.
    pub fn parse_sidecar(text: &str) -> Result<Self> {
        let get = |key: &str| -> Result<String> {
            text.lines()
                .filter_map(|l| l.split_once('='))
                .find(|(k, _)| k.trim() == key)
                .map(|(_, v)| v.trim().to_owned())
                .ok_or_else(|| Error::invalid_input(format!("Peek.app.info has no `{key}`")))
        };
        let bundle_version = get("bundle_version")?;
        let info = Self {
            bundle_id: get("bundle_id")?,
            bundle_version: bundle_version.parse().map_err(|_| {
                Error::invalid_input(format!(
                    "Peek.app.info bundle_version `{bundle_version}` is not an integer"
                ))
            })?,
            short_version: get("short_version")?,
            team_id: get("team_id").unwrap_or_default(),
            zip_sha256: get("zip_sha256")?,
            minimum_system_version: get("minimum_system_version")?,
        };
        info.validate()?;
        Ok(info)
    }

    /// Checks the fields an offer arrives with, wherever it comes from (a
    /// sidecar file, or a CLI's `app.offer` over IPC, which is plain serde):
    /// `zip_sha256` is 64 hex digits, and the text fields are short and free
    /// of control characters (they are written back as `key=value` lines).
    ///
    /// # Errors
    /// `invalid_input` naming the bad field.
    pub fn validate(&self) -> Result<()> {
        if self.zip_sha256.len() != 64 || !self.zip_sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::invalid_input(
                "Peek.app.info zip_sha256 is not 64 hex digits",
            ));
        }
        for (key, value) in [
            ("bundle_id", &self.bundle_id),
            ("short_version", &self.short_version),
            ("team_id", &self.team_id),
            ("minimum_system_version", &self.minimum_system_version),
        ] {
            if value.len() > 256 || value.chars().any(char::is_control) {
                return Err(Error::invalid_input(format!(
                    "Peek.app.info {key} must be at most 256 characters without control characters"
                )));
            }
        }
        Ok(())
    }

    /// `CFBundleVersion` for a semantic version: `major*1_000_000 + minor*1_000 + patch`.
    ///
    /// # Errors
    /// `invalid_input` for anything but `MAJOR.MINOR.PATCH` with minor and
    /// patch below 1000.
    pub fn build_number(version: &str) -> Result<u64> {
        let parts: Vec<&str> = version.split('.').collect();
        let [major, minor, patch] = parts.as_slice() else {
            return Err(Error::invalid_input(format!(
                "`{version}` is not MAJOR.MINOR.PATCH"
            )));
        };
        let n = |s: &str| {
            s.parse::<u64>()
                .map_err(|_| Error::invalid_input(format!("`{version}` is not MAJOR.MINOR.PATCH")))
        };
        let (major, minor, patch) = (n(major)?, n(minor)?, n(patch)?);
        if minor >= 1000 || patch >= 1000 {
            return Err(Error::invalid_input(format!(
                "`{version}`: minor and patch must be below 1000"
            )));
        }
        Ok(major * 1_000_000 + minor * 1_000 + patch)
    }
}

/// `app.offer`: offer a bundled Peek.app to peekd's updater.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppOffer {
    /// Absolute path of `Peek.app.zip`.
    pub zip_path: String,
    /// Its sidecar.
    pub info: AppOfferInfo,
}

/// `app.offer` result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppOfferResult {
    /// Whether an update was scheduled.
    pub scheduled: bool,
    /// The installed build.
    pub installed_build: u64,
}

impl Op for AppOffer {
    const NAME: &'static str = "app.offer";
    type Output = AppOfferResult;
}

/// `daemon.status` (no auth).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonStatus {}

/// Peek.app state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiStatus {
    /// Connected.
    pub running: bool,
    /// Its build, when connected.
    #[serde(default)]
    pub build: Option<u64>,
}

/// `daemon.status` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonStatusResult {
    /// Always `true` when answering.
    pub running: bool,
    /// peekd's PID.
    pub pid: u32,
    /// peekd's version.
    pub version: String,
    /// The protocol it speaks.
    pub protocol: u32,
    /// The socket path.
    pub socket: String,
    /// Peek.app.
    pub ui: UiStatus,
    /// Homes attached.
    pub homes: u32,
}

impl Op for DaemonStatus {
    const NAME: &'static str = "daemon.status";
    type Output = DaemonStatusResult;
}

/// `app.uninstall` (CLI-local, beyond §1.6): peekd relays it to Peek.app,
/// which unregisters its login item and agent, recycles its bundle and
/// quits. The result is any JSON, e.g. `{"accepted":true,"via":"ui"}`. The
/// `auth` block is optional.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppUninstall {}

impl Op for AppUninstall {
    const NAME: &'static str = "app.uninstall";
    type Output = Value;
}

/// `doctor` (CLI-local, beyond §1.6): what only peekd and Peek.app know.
/// The result is read leniently; peekd answers at least
/// `{"ui_running":bool,"mic":"granted|denied|restricted|undetermined"|null,
///   "hotkeys":{"registered":[…],"failed":[…]}|null}`. The `auth` block is
/// optional.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Doctor {}

impl Op for Doctor {
    const NAME: &'static str = "doctor";
    type Output = Value;
}

/// `ask.result`: the final state of a `send --wait` ask, pushed on the CLI's
/// connection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AskResult {
    /// The ask.
    pub ask_id: AskId,
    /// `answered`, `dismissed`, `expired`, `cancelled` or `replaced`.
    pub state: AskState,
    /// The answer.
    #[serde(default)]
    pub answer: Option<Answer>,
    /// How.
    #[serde(default)]
    pub via: Option<AnswerVia>,
    /// The transcript of a voice answer.
    #[serde(default)]
    pub transcript: Option<String>,
    /// When.
    #[serde(default)]
    pub answered_at: Option<Timestamp>,
}

impl EventBody for AskResult {
    const NAME: &'static str = "ask.result";
}

/// `ask.result.ack`: sent by a `send --wait` CLI on the same connection once
/// it has read its `ask.result`. peekd counts the answer as delivered to the
/// CLI (so no ting is sent, D22) only after this arrives within
/// `waiter_ack`; a CLI that timed out, was interrupted or crashed before
/// acknowledging gets its answer by ting as usual. It is fire-and-forget: the
/// CLI does not wait for the reply.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskResultAck {
    /// The ask whose result was read.
    pub ask_id: AskId,
}

impl Op for AskResultAck {
    const NAME: &'static str = "ask.result.ack";
    type Output = Empty;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::Request;

    fn png() -> Vec<u8> {
        b"\x89PNG\r\n\x1a\n0000".to_vec()
    }

    fn show_with_blob(k: usize) -> Result<Show> {
        crate::json::from_value(
            json!({"elements":[{"type":"image","path":{"blob":k}}]}),
            "show",
        )
    }

    #[test]
    fn send_validation() -> Result<()> {
        let base = SendOp {
            isi: Some("deliberate".into()),
            speak: Some("hello".into()),
            show: None,
            ask: None,
            voice: None,
            lang: None,
            notify: vec![],
            duration_ms: None,
            expires_in_s: None,
            wait: false,
            expires_at: None,
            due_at: None,
            tz: None,
            replace: false,
        };
        base.validate(&[])?;
        let mut s = base.clone();
        s.speak = None;
        assert_eq!(
            s.validate(&[]).err().map(|e| e.code().clone()),
            Some(ErrorCode::NothingToSend)
        );

        let mut s = base.clone();
        s.show = Some(show_with_blob(0)?);
        s.validate(&[png()])?;
        assert!(s.validate(&[]).is_err(), "dangling blob reference");
        assert!(s.validate(&[png(), png()]).is_err(), "unreferenced blob");
        assert_eq!(
            s.validate(&[b"GIF00".to_vec()])
                .err()
                .map(|e| e.code().clone()),
            Some(ErrorCode::ImageUnsupported)
        );
        let mut big = png();
        big.resize(limits::IMAGE_MAX_BYTES + 1, 0);
        assert_eq!(
            s.validate(&[big]).err().map(|e| e.code().clone()),
            Some(ErrorCode::ImageTooLarge)
        );

        let mut s = base.clone();
        s.duration_ms = Some(1500);
        assert!(s.validate(&[]).is_err());
        s.duration_ms = Some(120_000);
        s.validate(&[])?;
        s.duration_ms = Some(121_000);
        assert!(s.validate(&[]).is_err());
        let mut s = base.clone();
        s.wait = true;
        assert_eq!(
            s.validate(&[]).err().map(|e| e.code().clone()),
            Some(ErrorCode::ConflictingFlags)
        );
        let mut s = base;
        s.isi = Some("x".repeat(161));
        assert!(s.validate(&[]).is_err());
        Ok(())
    }

    #[test]
    fn send_wire_shape() -> Result<()> {
        let op = SendOp {
            isi: None,
            speak: Some("hi".into()),
            show: None,
            ask: Some(crate::schema::ask::Ask::from_input(
                &json!({"question":"Keep?","type":"single_choice","options":["Yes","No"]}),
            )?),
            voice: None,
            lang: None,
            notify: vec![Notify::SpeechFinished],
            duration_ms: None,
            expires_in_s: Some(60),
            wait: true,
            expires_at: None,
            due_at: None,
            tz: None,
            replace: false,
        };
        let req = Request::new(&op, None, vec![])?;
        assert_eq!(req.fields["ask"]["type"], "single_choice");
        assert_eq!(
            req.fields["ask"]["options"][1],
            json!({"id":"2","label":"No"})
        );
        assert_eq!(req.fields["notify"], json!(["speech_finished"]));
        assert_eq!(req.parse::<SendOp>()?, op);
        // A plain op is byte-compatible with a 0.1.1 peekd: no new keys.
        for key in ["expires_at", "due_at", "tz", "replace"] {
            assert!(!req.fields.contains_key(key), "{key}");
        }
        // A full op round-trips.
        let mut full = op.clone();
        full.wait = false;
        full.expires_in_s = None;
        full.due_at = Some(Timestamp::parse("2026-09-27T12:30:00Z")?);
        full.expires_at = Some(Timestamp::parse("2026-09-27T13:00:00Z")?);
        full.tz = Some("Asia/Kolkata".into());
        full.replace = true;
        let req = Request::new(&full, None, vec![])?;
        assert_eq!(req.fields["due_at"], "2026-09-27T12:30:00.000Z");
        assert_eq!(req.fields["replace"], true);
        assert_eq!(req.parse::<SendOp>()?, full);
        assert_eq!(full.kind(), "speak+ask");
        Ok(())
    }

    fn speak_op() -> SendOp {
        SendOp {
            isi: None,
            speak: Some("hi".into()),
            show: None,
            ask: None,
            voice: None,
            lang: None,
            notify: vec![],
            duration_ms: None,
            expires_in_s: None,
            wait: false,
            expires_at: None,
            due_at: None,
            tz: None,
            replace: false,
        }
    }

    #[test]
    fn validate_at_deadlines() -> Result<()> {
        let now = Timestamp::parse("2026-09-27T06:12:00Z")?;
        let ms = |n: i64| Timestamp::from_unix_ms(now.unix_ms() + n);
        let s = 1000;
        let day = 86_400 * s;
        let check = |op: &SendOp, slack: i64| op.validate_at(&[], now, slack);
        let field = |r: Result<()>| {
            r.err().and_then(|e| {
                e.details()
                    .and_then(|d| d["field"].as_str().map(str::to_owned))
            })
        };
        let mut op = speak_op();
        // due_at, strict.
        op.due_at = Some(now);
        assert_eq!(field(check(&op, 0)).as_deref(), Some("--at"));
        op.due_at = Some(ms(1));
        check(&op, 0)?;
        op.due_at = Some(ms(365 * day));
        check(&op, 0)?;
        op.due_at = Some(ms(365 * day + 1));
        assert_eq!(field(check(&op, 0)).as_deref(), Some("--at"));
        // due_at with peekd's slack.
        op.due_at = Some(ms(-4 * s));
        check(&op, 5000)?;
        op.due_at = Some(ms(-6 * s));
        assert!(check(&op, 5000).is_err());
        // expires_at of a regular send.
        let mut op = speak_op();
        op.expires_at = Some(ms(9 * s));
        assert_eq!(field(check(&op, 0)).as_deref(), Some("--expires-at"));
        op.expires_at = Some(ms(10 * s));
        check(&op, 0)?;
        op.expires_at = Some(ms(7 * day));
        check(&op, 0)?;
        op.expires_at = Some(ms(7 * day + 1));
        assert!(check(&op, 0).is_err());
        op.expires_at = Some(ms(5 * s));
        check(&op, 5000)?;
        op.expires_at = Some(ms(4 * s));
        assert!(check(&op, 5000).is_err());
        // expires_at of a scheduled send: relative to due, no slack.
        let mut op = speak_op();
        op.due_at = Some(ms(3600 * s));
        op.expires_at = Some(ms(3600 * s + 9 * s));
        assert!(check(&op, 5000).is_err());
        op.expires_at = Some(ms(3600 * s + 10 * s));
        check(&op, 0)?;
        op.expires_at = Some(ms(3600 * s + 7 * day));
        check(&op, 0)?;
        op.expires_at = Some(ms(3600 * s + 7 * day + 1));
        assert!(check(&op, 5000).is_err());
        // expires_in_s is not for scheduled sends; tz is checked.
        let mut op = speak_op();
        op.due_at = Some(ms(60 * s));
        op.expires_in_s = Some(60);
        assert_eq!(
            check(&op, 0).err().map(|e| e.code().clone()),
            Some(ErrorCode::ConflictingFlags)
        );
        let mut op = speak_op();
        op.tz = Some("Asia/Kolkata".into());
        assert_eq!(
            check(&op, 0).err().map(|e| e.code().clone()),
            Some(ErrorCode::ConflictingFlags),
            "--tz needs --at or --expires-at"
        );
        op.due_at = Some(ms(60 * s));
        check(&op, 0)?;
        op.tz = Some("Asia Kolkata".into());
        assert_eq!(field(check(&op, 0)).as_deref(), Some("--tz"));
        op.tz = Some("x".repeat(65));
        assert!(check(&op, 0).is_err());
        // --expires-in on a speak is valid now; --replace with anything.
        let mut op = speak_op();
        op.expires_in_s = Some(900);
        op.replace = true;
        check(&op, 0)?;
        op.expires_in_s = Some(9);
        assert!(check(&op, 0).is_err());
        Ok(())
    }

    #[test]
    fn send_results_decode_old_replies_and_serialize_every_key() -> Result<()> {
        let e = |x: serde_json::Error| Error::internal(x.to_string());
        let sid = SendId::generate();
        let old: SendResult =
            serde_json::from_value(json!({"send_id": sid, "ask_id": null, "slot": 3,
            "status": "showing", "speech": null, "warnings": []}))
            .map_err(e)?;
        assert_eq!(old.queue_position, None);
        assert_eq!(old.schedule_id, None);
        let v = serde_json::to_value(&old).map_err(e)?;
        for key in [
            "queue_position",
            "waiting",
            "expires_at",
            "schedule_id",
            "due_at",
            "tz",
            "replaced_send_id",
        ] {
            assert!(v.get(key).is_some_and(Value::is_null), "{key}");
        }
        let scheduled: SendResult = serde_json::from_value(json!({"send_id": sid, "ask_id": null, "slot": 3,
            "status": "scheduled", "schedule_id": ScheduleId::generate(), "due_at": "2026-09-27T12:30:00Z",
            "tz": "Asia/Kolkata"}))
        .map_err(e)?;
        assert_eq!(scheduled.status, SendStatus::Scheduled);
        assert_eq!(scheduled.tz.as_deref(), Some("Asia/Kolkata"));
        Ok(())
    }

    #[test]
    fn hello_results_carry_features() -> Result<()> {
        let e = |x: serde_json::Error| Error::internal(x.to_string());
        let old: HelloResult =
            serde_json::from_value(json!({"protocol":1,"peekd_version":"0.1.1"})).map_err(e)?;
        assert!(old.features.is_empty());
        assert!(!old.has_feature(features::QUEUE_V2));
        let new = HelloResult {
            protocol: 1,
            peekd_version: "0.1.2".into(),
            app: None,
            features: features::ALL.iter().map(|f| (*f).to_owned()).collect(),
        };
        let v = serde_json::to_value(&new).map_err(e)?;
        assert_eq!(
            v["features"],
            json!([
                "queue_v2",
                "expiry_all",
                "replace",
                "schedule",
                "notify_shown"
            ])
        );
        assert!(new.has_feature(features::SCHEDULE));
        Ok(())
    }

    #[test]
    fn queue_and_schedule_ops() -> Result<()> {
        let e = |x: serde_json::Error| Error::internal(x.to_string());
        assert_eq!(QueueList::NAME, "queue.list");
        assert_eq!(QueueClear::NAME, "queue.clear");
        assert_eq!(SendCancel::NAME, "send.cancel");
        assert_eq!(ScheduleList::NAME, "schedule.list");
        assert_eq!(ScheduleCancel::NAME, "schedule.cancel");
        assert_eq!(ScheduleClear::NAME, "schedule.clear");
        let req = Request::new(&QueueClear { all: true }, None, vec![])?;
        assert_eq!(req.parse::<QueueClear>()?, QueueClear { all: true });
        let bare: QueueClear = serde_json::from_value(json!({})).map_err(e)?;
        assert!(!bare.all);
        let sid = SendId::generate();
        for id in [
            sid.to_string(),
            AskId::generate().to_string(),
            ScheduleId::generate().to_string(),
        ] {
            let req = Request::new(&SendCancel { target: id.clone() }, None, vec![])?;
            assert_eq!(req.fields["target"], id.as_str());
            assert_eq!(req.parse::<SendCancel>()?.target, id);
            let req = Request::new(&ScheduleCancel { target: id.clone() }, None, vec![])?;
            assert_eq!(req.parse::<ScheduleCancel>()?.target, id);
        }
        let item = json!({"send_id": sid, "ask_id": null, "kind": "show", "summary": "Build finished",
            "state": "on_screen", "queue_position": 0, "created_at": "2026-09-27T12:00:00Z",
            "queued_at": "2026-09-27T12:00:00Z", "age_ms": 12000, "expires_at": null,
            "shown_at": "2026-09-27T12:00:01Z", "schedule_id": null, "due_at": null});
        let list: QueueListResult = serde_json::from_value(json!({"slot": 3, "limit": 5,
            "on_screen": item, "waiting": [], "held": null, "scheduled": 4, "scheduled_limit": 500}))
        .map_err(e)?;
        assert_eq!(
            list.on_screen.as_ref().map(|i| i.state),
            Some(QueueItemState::OnScreen)
        );
        let back = serde_json::to_value(&list).map_err(e)?;
        assert_eq!(back["on_screen"]["state"], "on_screen");
        assert_eq!(back["held"], Value::Null);
        let due: QueueItemState = serde_json::from_value(json!("due_waiting")).map_err(e)?;
        assert_eq!(due, QueueItemState::DueWaiting);
        let sched: ScheduleListResult =
            serde_json::from_value(json!({"limit": 500, "scheduled": [{
            "schedule_id": ScheduleId::generate(), "send_id": sid, "ask_id": null, "kind": "ask",
            "summary": "Stand-up in 5?", "due_at": "2026-09-27T12:30:00Z", "tz": "Asia/Kolkata",
            "expires_at": null, "replace": true, "created_at": "2026-09-27T10:00:00Z"}]}))
            .map_err(e)?;
        assert!(sched.scheduled[0].replace);
        let cancelled: SendCancelResult =
            serde_json::from_value(json!({"send_id": sid, "ask_id": null,
            "schedule_id": null, "was": "closed", "queue_position": null, "state": "speech_done"}))
            .map_err(e)?;
        assert_eq!(cancelled.was, CancelledFrom::Closed);
        let status: QueueStatus = serde_json::from_value(json!({"pending": 2})).map_err(e)?;
        assert_eq!(status.limit, 5, "an older peekd's status still reads");
        assert_eq!(QueueStatus::default().limit, 5);
        let held: HeldReason = serde_json::from_value(json!("app_not_running")).map_err(e)?;
        assert_eq!(held, HeldReason::AppNotRunning);
        let replaced: AskState = serde_json::from_value(json!("replaced")).map_err(e)?;
        assert_eq!(replaced, AskState::Replaced);
        let unreg: UnregisterResult =
            serde_json::from_value(json!({"released_slot": 3, "cancelled_asks": []})).map_err(e)?;
        assert!(unreg.cancelled_sends.is_empty() && unreg.cancelled_scheduled.is_empty());
        let hist: HistoryItem = serde_json::from_value(json!({"send_id": sid, "kind": "show",
            "created_at": "2026-09-27T12:00:00Z"}))
        .map_err(e)?;
        assert!(hist.shown_at.is_none());
        let v = serde_json::to_value(&hist).map_err(e)?;
        assert!(v.get("schedule_id").is_none(), "absent unless set");
        Ok(())
    }

    #[test]
    fn hello_shapes() -> Result<()> {
        let v = serde_json::to_value(Hello::cli("macos-aarch64", None))
            .map_err(|e| Error::internal(e.to_string()))?;
        assert_eq!(
            v,
            json!({"role":"cli","cli_version":crate::VERSION,"protocols":[1],"platform":"macos-aarch64"})
        );
        let ui: Hello = serde_json::from_value(json!({"role":"ui","app_build":1000,"app_version":"0.1.0","protocols":[1],"extra":true}))
            .map_err(|e| Error::internal(e.to_string()))?;
        assert_eq!(ui.protocols(), &[1]);
        Ok(())
    }

    #[test]
    fn sidecar_and_build_numbers() -> Result<()> {
        let info = AppOfferInfo::parse_sidecar(&format!(
            "bundle_id=ai.tos.peek\nbundle_version=1000\nshort_version=0.1.0\nteam_id=LTBSK59BJ2\nzip_sha256={}\nminimum_system_version=26.0\n",
            "a".repeat(64)
        ))?;
        assert_eq!(info.bundle_version, 1000);
        assert!(AppOfferInfo::parse_sidecar("bundle_id=x\nbundle_version=abc").is_err());
        assert_eq!(AppOfferInfo::build_number("0.1.0")?, 1000);
        assert_eq!(AppOfferInfo::build_number("1.2.3")?, 1_002_003);
        assert!(AppOfferInfo::build_number("1.2").is_err());
        assert!(AppOfferInfo::build_number("1.1000.0").is_err());
        Ok(())
    }

    #[test]
    fn results_decode_leniently() -> Result<()> {
        let r: StatusResult = serde_json::from_value(json!({"slot":{"index":3,"side":"right"},"drawing":null,
            "queue":{"pending":0},"pending_asks":1,"deliveries":{"pending":0,"authority_required":0,"last_error":null},
            "ui_running":true,"future":1})).map_err(|e| Error::internal(e.to_string()))?;
        assert_eq!(r.slot.map(|s| s.index.get()), Some(3));
        assert_eq!(r.carbon, None, "older peekd has no presence");
        let r: StatusResult = serde_json::from_value(json!({"slot":null,"drawing":null,
            "queue":{"pending":2},"pending_asks":0,"deliveries":{"pending":0,"authority_required":0,"last_error":null},
            "ui_running":true,"carbon":{"available":false,"reason":"locked"}})).map_err(|e| Error::internal(e.to_string()))?;
        assert_eq!(
            r.carbon,
            Some(CarbonStatus {
                available: false,
                reason: crate::ipc::ui::PresenceReason::Locked,
                paused: false,
            })
        );
        let h = History {
            limit: Some(201),
            before: None,
        };
        assert!(h.validate().is_err());
        Ok(())
    }
}
