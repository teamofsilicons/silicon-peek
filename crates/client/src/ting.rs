//! Ting payloads peek sends (BLUEPRINT §3.1–§3.5): the nine `peek.*` data
//! schemas, the delivery request peekd posts to peek-server, and the exact
//! Ting request bodies peek-server builds from it.
//!
//! Every payload carries `"schema":1`, `"slot":1..8` and
//! `"context":"production"|"testing"`. Schemas only ever change additively.
//! peek-server parses them with `deny_unknown_fields`, so an unknown field is
//! refused rather than forwarded.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Value, json};

use crate::{
    APP_ID, VERSION,
    error::{Error, ErrorCode, Result},
    identity::{ActorId, DataContext, OrgId, SlotIndex},
    ids::{AskId, EventId, MessageId, ScheduleId, SendId, TingKeyEvent, ting_key},
    schema::ask::{Answer, AskType},
    timestamp::Timestamp,
};

/// Ting's request body limit; peek bodies must stay well under it.
pub const TING_BODY_MAX_BYTES: usize = 256 * 1024;

/// The payload schema marker: always serializes as `1`, and only `1` parses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SchemaV1;

impl Serialize for SchemaV1 {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_u8(1)
    }
}

impl<'de> Deserialize<'de> for SchemaV1 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        match u64::deserialize(d)? {
            1 => Ok(Self),
            n => Err(serde::de::Error::custom(format!(
                "payload schema {n} is not supported by this peek (it speaks schema 1)"
            ))),
        }
    }
}

/// The nine Ting types peek defines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TingType {
    /// A Carbon answered an ask.
    #[serde(rename = "peek.ask.answered")]
    AskAnswered,
    /// A Carbon dismissed an ask without answering.
    #[serde(rename = "peek.ask.dismissed")]
    AskDismissed,
    /// An ask reached its `--expires-in` deadline.
    #[serde(rename = "peek.ask.expired")]
    AskExpired,
    /// A Carbon spoke or typed to the Silicon with no pending ask.
    #[serde(rename = "peek.message.received")]
    MessageReceived,
    /// A `--speak` finished or was stopped (opt-in).
    #[serde(rename = "peek.speech.finished")]
    SpeechFinished,
    /// A Carbon closed a `--show` early (opt-in).
    #[serde(rename = "peek.show.dismissed")]
    ShowDismissed,
    /// A `--speak` or `--show` passed its deadline before it finished.
    #[serde(rename = "peek.send.expired")]
    SendExpired,
    /// A scheduled send (`--in`/`--at`) came due.
    #[serde(rename = "peek.schedule.due")]
    ScheduleDue,
    /// A send appeared on screen (scheduled sends always; opt-in otherwise).
    #[serde(rename = "peek.send.shown")]
    SendShown,
}

impl TingType {
    /// Every type, in registration order.
    pub const ALL: [TingType; 9] = [
        TingType::AskAnswered,
        TingType::AskDismissed,
        TingType::AskExpired,
        TingType::MessageReceived,
        TingType::SpeechFinished,
        TingType::ShowDismissed,
        TingType::SendExpired,
        TingType::ScheduleDue,
        TingType::SendShown,
    ];

    /// The Ting type name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AskAnswered => "peek.ask.answered",
            Self::AskDismissed => "peek.ask.dismissed",
            Self::AskExpired => "peek.ask.expired",
            Self::MessageReceived => "peek.message.received",
            Self::SpeechFinished => "peek.speech.finished",
            Self::ShowDismissed => "peek.show.dismissed",
            Self::SendExpired => "peek.send.expired",
            Self::ScheduleDue => "peek.schedule.due",
            Self::SendShown => "peek.send.shown",
        }
    }

    /// Parses a type name (the delivery allowlist).
    ///
    /// # Errors
    /// `invalid_input` for any other type.
    pub fn parse(s: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|t| t.as_str() == s)
            .ok_or_else(|| {
                Error::invalid_input(format!(
                    "`{s}` is not a peek Ting type; allowed: {}",
                    Self::ALL.map(TingType::as_str).join(", ")
                ))
            })
    }

    /// The registered description (byte-identical in every context, §3.1).
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::AskAnswered => "A Carbon answered a peek --ask (voice, keyboard or click).",
            Self::AskDismissed => "A Carbon dismissed a peek --ask without answering.",
            Self::AskExpired => "A peek --ask reached its --expires-in deadline unanswered.",
            Self::MessageReceived => {
                "A Carbon spoke or typed to the Silicon from its peek with no pending ask."
            }
            Self::SpeechFinished => "A peek --speak finished playing or was stopped by the Carbon.",
            Self::ShowDismissed => "A Carbon closed a peek --show before it retracted.",
            Self::SendExpired => {
                "A peek --speak or --show reached its --expires-in or --expires-at deadline before it finished; the data says whether it was shown."
            }
            Self::ScheduleDue => {
                "A scheduled peek send (--in or --at) came due; the data says whether it was shown, queued, expired or replaced the active peek."
            }
            Self::SendShown => {
                "A peek send appeared on screen for the Carbon (always for scheduled sends; opt-in with --notify shown)."
            }
        }
    }

    /// The last segment of this type's Ting key.
    #[must_use]
    pub const fn key_event(self) -> TingKeyEvent {
        match self {
            Self::AskAnswered => TingKeyEvent::Answered,
            Self::AskDismissed => TingKeyEvent::Dismissed,
            Self::AskExpired => TingKeyEvent::Expired,
            Self::MessageReceived => TingKeyEvent::Message,
            Self::SpeechFinished => TingKeyEvent::SpeechFinished,
            Self::ShowDismissed => TingKeyEvent::ShowDismissed,
            Self::SendExpired => TingKeyEvent::SendExpired,
            Self::ScheduleDue => TingKeyEvent::Due,
            Self::SendShown => TingKeyEvent::Shown,
        }
    }
}

/// How an answer was given.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnswerVia {
    /// Spoken, then transcribed once by Deepgram.
    Voice,
    /// Typed.
    Keyboard,
    /// Clicked.
    Click,
}

/// How a Carbon message was given.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageVia {
    /// Spoken, then transcribed.
    Voice,
    /// Typed.
    Keyboard,
}

/// The gesture that closed a bubble.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Gesture {
    /// Escape.
    Esc,
    /// A single click on the down arrow (audio keeps playing).
    DownArrow,
    /// A double click on the down arrow (audio stops too).
    DownArrowDouble,
    /// Two Escs within 0.4 s on a show or speak (audio stops too).
    EscDouble,
}

/// `peek.ask.answered` data.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskAnswered {
    /// Always 1.
    pub schema: SchemaV1,
    /// The ask.
    pub ask_id: AskId,
    /// The send that carried it.
    pub send_id: SendId,
    /// The question, verbatim.
    pub question: String,
    /// The ask type.
    pub ask_type: AskType,
    /// The answer.
    pub answer: Answer,
    /// How it was given.
    pub via: AnswerVia,
    /// The final transcript, only when `via` is `voice`.
    pub transcript: Option<String>,
    /// When the ask was shown.
    pub asked_at: Timestamp,
    /// When it was answered.
    pub answered_at: Timestamp,
    /// The slot.
    pub slot: SlotIndex,
    /// Production or testing.
    pub context: DataContext,
}

/// `peek.ask.dismissed` data.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskDismissed {
    /// Always 1.
    pub schema: SchemaV1,
    /// The ask.
    pub ask_id: AskId,
    /// The send that carried it.
    pub send_id: SendId,
    /// The question, verbatim.
    pub question: String,
    /// The ask type.
    pub ask_type: AskType,
    /// How it was closed.
    pub gesture: Gesture,
    /// When the ask was shown.
    pub asked_at: Timestamp,
    /// When it was dismissed.
    pub dismissed_at: Timestamp,
    /// The slot.
    pub slot: SlotIndex,
    /// Production or testing.
    pub context: DataContext,
}

/// `peek.ask.expired` data.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskExpired {
    /// Always 1.
    pub schema: SchemaV1,
    /// The ask.
    pub ask_id: AskId,
    /// The send that carried it.
    pub send_id: SendId,
    /// The question, verbatim.
    pub question: String,
    /// The ask type.
    pub ask_type: AskType,
    /// When the ask was shown.
    pub asked_at: Timestamp,
    /// When it expired.
    pub expired_at: Timestamp,
    /// The slot.
    pub slot: SlotIndex,
    /// Production or testing.
    pub context: DataContext,
    /// Whether it was ever on screen (additive: absent from peekd 0.1.1;
    /// peekd 0.1.2 always sends it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shown: Option<bool>,
}

/// `peek.message.received` data.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageReceived {
    /// Always 1.
    pub schema: SchemaV1,
    /// The message.
    pub message_id: MessageId,
    /// What the Carbon said or typed.
    pub text: String,
    /// How.
    pub via: MessageVia,
    /// When it was sent.
    pub sent_at: Timestamp,
    /// The slot.
    pub slot: SlotIndex,
    /// Production or testing.
    pub context: DataContext,
    /// The most recent send in that slot, if any.
    pub in_reply_to: Option<SendId>,
}

/// `peek.speech.finished` data.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpeechFinished {
    /// Always 1.
    pub schema: SchemaV1,
    /// The send.
    pub send_id: SendId,
    /// Whether the Carbon stopped it (double click on the down arrow).
    pub stopped_by_user: bool,
    /// Milliseconds played.
    pub played_ms: u64,
    /// Total milliseconds of audio.
    pub total_ms: u64,
    /// When it finished.
    pub finished_at: Timestamp,
    /// The slot.
    pub slot: SlotIndex,
    /// Production or testing.
    pub context: DataContext,
}

/// `peek.show.dismissed` data.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShowDismissed {
    /// Always 1.
    pub schema: SchemaV1,
    /// The send.
    pub send_id: SendId,
    /// How it was closed.
    pub gesture: Gesture,
    /// Milliseconds it was visible.
    pub visible_ms: u64,
    /// When it was dismissed.
    pub dismissed_at: Timestamp,
    /// The slot.
    pub slot: SlotIndex,
    /// Production or testing.
    pub context: DataContext,
}

/// The five send kinds a payload may name.
pub const SEND_KINDS: [&str; 5] = ["speak", "show", "ask", "speak+show", "speak+ask"];

/// `peek.send.expired` data: a speak/show send (never an ask) passed its
/// deadline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SendExpired {
    /// Always 1.
    pub schema: SchemaV1,
    /// The send.
    pub send_id: SendId,
    /// `speak`, `show` or `speak+show`.
    pub kind: String,
    /// When `peek send` ran.
    pub created_at: Timestamp,
    /// The deadline the Silicon set.
    pub expires_at: Timestamp,
    /// When peekd expired it (at or after `expires_at`; later after sleep).
    pub expired_at: Timestamp,
    /// Whether it was ever on screen.
    pub shown: bool,
    /// When it appeared; null exactly when it was never shown.
    pub shown_at: Option<Timestamp>,
    /// Whether it came from `--in`/`--at`.
    pub scheduled: bool,
    /// The scheduled send; null exactly when it was not scheduled.
    pub schedule_id: Option<ScheduleId>,
    /// The slot.
    pub slot: SlotIndex,
    /// Production or testing.
    pub context: DataContext,
}

/// What happened when a scheduled send came due.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DueOutcome {
    /// It became the active bubble and was handed to Peek.app.
    Shown,
    /// It waits (behind others, for room, or while the Carbon is away).
    Queued,
    /// Its `--expires-at` had passed; it was never shown.
    Expired,
    /// Its `--replace` took over the active peek.
    Replaced,
}

/// Why a due scheduled send is not on screen yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitingReason {
    /// Other sends of this Silicon are ahead of it.
    BehindOthers,
    /// Five sends already wait; it takes the next free spot.
    QueueFull,
    /// The Carbon's screen is locked or asleep.
    CarbonAway,
    /// The Carbon paused Peek.
    Paused,
    /// Peek.app is not running.
    AppNotRunning,
}

/// `peek.schedule.due` data: sent once, when peekd fires the scheduled send
/// (at its due time, or on catch-up after sleep or downtime).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduleDue {
    /// Always 1.
    pub schema: SchemaV1,
    /// The scheduled send.
    pub schedule_id: ScheduleId,
    /// The send it became.
    pub send_id: SendId,
    /// Its ask.
    pub ask_id: Option<AskId>,
    /// `speak`, `show`, `ask`, `speak+show` or `speak+ask`.
    pub kind: String,
    /// When it was due.
    pub due_at: Timestamp,
    /// When peekd fired it (at or after `due_at`).
    pub fired_at: Timestamp,
    /// What happened.
    pub outcome: DueOutcome,
    /// The send it replaced; set exactly when `outcome` is `replaced`.
    pub replaced_send_id: Option<SendId>,
    /// Why it waits; set when `outcome` is `queued` (or `replaced` but held).
    pub waiting_reason: Option<WaitingReason>,
    /// Set exactly when `outcome` is `queued`: 0 = current but held, 1–5
    /// waiting, 6 and up waiting for room.
    pub queue_position: Option<u32>,
    /// Its deadline.
    pub expires_at: Option<Timestamp>,
    /// The slot.
    pub slot: SlotIndex,
    /// Production or testing.
    pub context: DataContext,
}

/// `peek.send.shown` data: the send began to appear on screen.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SendShown {
    /// Always 1.
    pub schema: SchemaV1,
    /// The send.
    pub send_id: SendId,
    /// Its ask.
    pub ask_id: Option<AskId>,
    /// `speak`, `show`, `ask`, `speak+show` or `speak+ask`.
    pub kind: String,
    /// When `peek send` ran.
    pub created_at: Timestamp,
    /// When it appeared.
    pub shown_at: Timestamp,
    /// Whether it came from `--in`/`--at`.
    pub scheduled: bool,
    /// The scheduled send; null exactly when it was not scheduled.
    pub schedule_id: Option<ScheduleId>,
    /// The slot.
    pub slot: SlotIndex,
    /// Production or testing.
    pub context: DataContext,
}

/// Any of the nine payloads.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum TingData {
    /// `peek.ask.answered`.
    AskAnswered(AskAnswered),
    /// `peek.ask.dismissed`.
    AskDismissed(AskDismissed),
    /// `peek.ask.expired`.
    AskExpired(AskExpired),
    /// `peek.message.received`.
    MessageReceived(MessageReceived),
    /// `peek.speech.finished`.
    SpeechFinished(SpeechFinished),
    /// `peek.show.dismissed`.
    ShowDismissed(ShowDismissed),
    /// `peek.send.expired`.
    SendExpired(SendExpired),
    /// `peek.schedule.due`.
    ScheduleDue(ScheduleDue),
    /// `peek.send.shown`.
    SendShown(SendShown),
}

impl TingData {
    /// The payload's type.
    #[must_use]
    pub const fn ting_type(&self) -> TingType {
        match self {
            Self::AskAnswered(_) => TingType::AskAnswered,
            Self::AskDismissed(_) => TingType::AskDismissed,
            Self::AskExpired(_) => TingType::AskExpired,
            Self::MessageReceived(_) => TingType::MessageReceived,
            Self::SpeechFinished(_) => TingType::SpeechFinished,
            Self::ShowDismissed(_) => TingType::ShowDismissed,
            Self::SendExpired(_) => TingType::SendExpired,
            Self::ScheduleDue(_) => TingType::ScheduleDue,
            Self::SendShown(_) => TingType::SendShown,
        }
    }

    /// The ID the Ting key is built from: the ask, message, send or
    /// scheduled send.
    #[must_use]
    pub fn subject_id(&self) -> &str {
        match self {
            Self::AskAnswered(d) => d.ask_id.as_str(),
            Self::AskDismissed(d) => d.ask_id.as_str(),
            Self::AskExpired(d) => d.ask_id.as_str(),
            Self::MessageReceived(d) => d.message_id.as_str(),
            Self::SpeechFinished(d) => d.send_id.as_str(),
            Self::ShowDismissed(d) => d.send_id.as_str(),
            Self::SendExpired(d) => d.send_id.as_str(),
            Self::ScheduleDue(d) => d.schedule_id.as_str(),
            Self::SendShown(d) => d.send_id.as_str(),
        }
    }

    /// Parses `data` strictly as the schema of `ting_type`.
    ///
    /// # Errors
    /// `invalid_input` with serde's explanation (unknown field, wrong type,
    /// unsupported schema).
    pub fn from_value(ting_type: TingType, data: Value) -> Result<Self> {
        let what = format!("{} data", ting_type.as_str());
        let bad = |e: serde_json::Error| Error::invalid_input(format!("{what} is not valid: {e}"));
        Ok(match ting_type {
            TingType::AskAnswered => Self::AskAnswered(serde_json::from_value(data).map_err(bad)?),
            TingType::AskDismissed => {
                Self::AskDismissed(serde_json::from_value(data).map_err(bad)?)
            }
            TingType::AskExpired => Self::AskExpired(serde_json::from_value(data).map_err(bad)?),
            TingType::MessageReceived => {
                Self::MessageReceived(serde_json::from_value(data).map_err(bad)?)
            }
            TingType::SpeechFinished => {
                Self::SpeechFinished(serde_json::from_value(data).map_err(bad)?)
            }
            TingType::ShowDismissed => {
                Self::ShowDismissed(serde_json::from_value(data).map_err(bad)?)
            }
            TingType::SendExpired => Self::SendExpired(serde_json::from_value(data).map_err(bad)?),
            TingType::ScheduleDue => Self::ScheduleDue(serde_json::from_value(data).map_err(bad)?),
            TingType::SendShown => Self::SendShown(serde_json::from_value(data).map_err(bad)?),
        })
    }

    /// Semantic checks serde cannot express (transcript only for voice,
    /// non-empty texts, answer kind matching `ask_type`, the nullable fields
    /// of the 0.1.2 types agreeing with their flags and outcome).
    ///
    /// # Errors
    /// `invalid_input`.
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::AskAnswered(d) => {
                if d.answer.ask_type() != d.ask_type {
                    return Err(Error::invalid_input(format!(
                        "answer kind {} does not match ask_type {}",
                        d.answer.ask_type().as_str(),
                        d.ask_type.as_str()
                    )));
                }
                if d.transcript.is_some() && d.via != AnswerVia::Voice {
                    return Err(Error::invalid_input(
                        "transcript is only sent with voice answers",
                    ));
                }
                if d.question.is_empty() {
                    return Err(Error::invalid_input("question is empty"));
                }
            }
            Self::MessageReceived(d) => {
                if d.text.trim().is_empty() {
                    return Err(Error::invalid_input(
                        "a Carbon message must not be empty; an empty answer is never sent",
                    ));
                }
            }
            Self::SendExpired(d) => {
                if !matches!(d.kind.as_str(), "speak" | "show" | "speak+show") {
                    return Err(Error::invalid_input(format!(
                        "peek.send.expired kind `{}` must be speak, show or speak+show (asks send peek.ask.expired)",
                        d.kind
                    )));
                }
                if d.shown != d.shown_at.is_some() {
                    return Err(Error::invalid_input(
                        "peek.send.expired shown_at must be set exactly when shown is true",
                    ));
                }
                if d.scheduled != d.schedule_id.is_some() {
                    return Err(Error::invalid_input(
                        "peek.send.expired schedule_id must be set exactly when scheduled is true",
                    ));
                }
                if d.expired_at < d.created_at {
                    return Err(Error::invalid_input(
                        "peek.send.expired expired_at is before created_at",
                    ));
                }
            }
            Self::ScheduleDue(d) => {
                check_kind("peek.schedule.due", &d.kind)?;
                if d.replaced_send_id.is_some() != (d.outcome == DueOutcome::Replaced) {
                    return Err(Error::invalid_input(
                        "peek.schedule.due replaced_send_id must be set exactly when outcome is replaced",
                    ));
                }
                if d.queue_position.is_some() != (d.outcome == DueOutcome::Queued) {
                    return Err(Error::invalid_input(
                        "peek.schedule.due queue_position must be set exactly when outcome is queued",
                    ));
                }
                match d.outcome {
                    DueOutcome::Shown | DueOutcome::Expired if d.waiting_reason.is_some() => {
                        return Err(Error::invalid_input(
                            "peek.schedule.due waiting_reason must be null when outcome is shown or expired",
                        ));
                    }
                    DueOutcome::Queued if d.waiting_reason.is_none() => {
                        return Err(Error::invalid_input(
                            "peek.schedule.due waiting_reason must be set when outcome is queued",
                        ));
                    }
                    _ => {}
                }
                if d.fired_at < d.due_at {
                    return Err(Error::invalid_input(
                        "peek.schedule.due fired_at is before due_at",
                    ));
                }
            }
            Self::SendShown(d) => {
                check_kind("peek.send.shown", &d.kind)?;
                if d.scheduled != d.schedule_id.is_some() {
                    return Err(Error::invalid_input(
                        "peek.send.shown schedule_id must be set exactly when scheduled is true",
                    ));
                }
                if d.shown_at < d.created_at {
                    return Err(Error::invalid_input(
                        "peek.send.shown shown_at is before created_at",
                    ));
                }
            }
            Self::AskDismissed(_)
            | Self::AskExpired(_)
            | Self::SpeechFinished(_)
            | Self::ShowDismissed(_) => {}
        }
        Ok(())
    }
}

fn check_kind(ting: &str, kind: &str) -> Result<()> {
    if SEND_KINDS.contains(&kind) {
        Ok(())
    } else {
        Err(Error::invalid_input(format!(
            "{ting} kind `{kind}` must be one of {}",
            SEND_KINDS.join(", ")
        )))
    }
}

/// Ting `metadata`: `{"isi"?, "peek_version"}`; `isi` is omitted when unknown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TingMetadata {
    /// The ISI of the send (or of the slot's most recent send, for messages).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isi: Option<String>,
    /// The peek version that recorded the event.
    pub peek_version: String,
}

impl TingMetadata {
    /// Metadata for this build.
    #[must_use]
    pub fn new(isi: Option<String>) -> Self {
        Self {
            isi,
            peek_version: VERSION.to_owned(),
        }
    }

    /// Checks `isi` (≤ 160 characters, no control characters) and the version.
    ///
    /// # Errors
    /// `invalid_input`.
    pub fn validate(&self) -> Result<()> {
        if let Some(isi) = &self.isi {
            crate::schema::send::check_isi(isi)?;
        }
        if self.peek_version.is_empty() || self.peek_version.len() > 32 {
            return Err(Error::invalid_input(
                "metadata.peek_version must be 1–32 characters",
            ));
        }
        Ok(())
    }
}

/// The body peekd posts to `POST /api/v1/deliveries` (BLUEPRINT §3.5). Its
/// bytes are fixed when the outbox row is created and never re-serialized.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryRequest {
    /// The outbox event ID (also the idempotency suffix).
    pub event_id: EventId,
    /// The Ting type.
    #[serde(rename = "type")]
    pub ting_type: TingType,
    /// `"<actor_id>/<subject_id>/<event>"`.
    pub key: String,
    /// The payload for `ting_type`.
    pub data: Value,
    /// `{"isi"?, "peek_version"}`.
    pub metadata: TingMetadata,
}

impl DeliveryRequest {
    /// Builds a delivery for `actor` from typed data, with a fresh event ID.
    ///
    /// # Errors
    /// `invalid_input` when the data fails its semantic checks or the key is
    /// too long.
    pub fn new(actor: &ActorId, data: &TingData, metadata: TingMetadata) -> Result<Self> {
        data.validate()?;
        metadata.validate()?;
        let ting_type = data.ting_type();
        let key = ting_key(actor, data.subject_id(), ting_type.key_event())?;
        let data = serde_json::to_value(data)
            .map_err(|e| Error::internal(format!("serializing Ting data failed: {e}")))?;
        Ok(Self {
            event_id: EventId::generate(),
            ting_type,
            key,
            data,
            metadata,
        })
    }

    /// The exact request bytes (compact JSON, fields in declaration order).
    ///
    /// # Errors
    /// `internal_error` if serialization fails.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(self)
            .map_err(|e| Error::internal(format!("serializing a delivery failed: {e}")))
    }

    /// peek-server's validation (§3.5 step 2): the type is allowlisted, the
    /// key belongs to the verified actor and names this event, `data` matches
    /// the type's schema, and `metadata` holds only `isi` and `peek_version`.
    ///
    /// # Errors
    /// `invalid_input` naming the rule that failed.
    pub fn validate_for(&self, verified_actor: &ActorId) -> Result<TingData> {
        let data = TingData::from_value(self.ting_type, self.data.clone())?;
        data.validate()?;
        self.metadata.validate()?;
        let expected = ting_key(
            verified_actor,
            data.subject_id(),
            self.ting_type.key_event(),
        )?;
        if self.key != expected {
            return Err(Error::invalid_input(format!(
                "delivery key `{}` does not match `{expected}`; keys are <verified actor>/<subject id>/<event>",
                self.key
            )));
        }
        Ok(data)
    }
}

/// The Ting `POST /v1/tings` body, built by peek-server only from the verified
/// identity and the validated delivery. Field order is fixed:
/// `org_id, type, for, key, data, metadata`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TingSend<'a> {
    /// The verified org handle.
    pub org_id: &'a OrgId,
    /// The Ting type.
    #[serde(rename = "type")]
    pub ting_type: TingType,
    /// The recipient: always the verified actor.
    #[serde(rename = "for")]
    pub recipient: &'a ActorId,
    /// The delivery key.
    pub key: &'a str,
    /// The payload.
    pub data: &'a Value,
    /// The metadata.
    pub metadata: &'a TingMetadata,
}

impl TingSend<'_> {
    /// Deterministic body bytes, refused above 256 KiB.
    ///
    /// # Errors
    /// `invalid_input` when the body is too large.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let bytes = serde_json::to_vec(self)
            .map_err(|e| Error::internal(format!("serializing a Ting body failed: {e}")))?;
        if bytes.len() > TING_BODY_MAX_BYTES {
            return Err(Error::new(
                ErrorCode::PayloadTooLarge,
                format!(
                    "the Ting body is {} bytes; Ting accepts at most {TING_BODY_MAX_BYTES}",
                    bytes.len()
                ),
            ));
        }
        Ok(bytes)
    }
}

/// Builds the exact `/v1/tings` body bytes for a validated delivery.
///
/// # Errors
/// As [`DeliveryRequest::validate_for`] and [`TingSend::to_bytes`].
pub fn ting_send_body(org: &OrgId, actor: &ActorId, delivery: &DeliveryRequest) -> Result<Vec<u8>> {
    delivery.validate_for(actor)?;
    TingSend {
        org_id: org,
        ting_type: delivery.ting_type,
        recipient: actor,
        key: &delivery.key,
        data: &delivery.data,
        metadata: &delivery.metadata,
    }
    .to_bytes()
}

/// `POST /v1/subscriptions` body: exactly `{"org_id","app_id":"peek","for"}`.
///
/// # Errors
/// `internal_error` if serialization fails.
pub fn subscription_register_body(org: &OrgId, actor: &ActorId) -> Result<Vec<u8>> {
    serde_json::to_vec(&json!({"org_id": org, "app_id": APP_ID, "for": actor}))
        .map_err(|e| Error::internal(format!("serializing a subscription failed: {e}")))
}

/// `POST /v1/subscriptions/revoke` body: exactly `{"org_id","id"}`.
///
/// # Errors
/// `internal_error` if serialization fails.
pub fn subscription_revoke_body(org: &OrgId, subscription_id: &str) -> Result<Vec<u8>> {
    serde_json::to_vec(&json!({"org_id": org, "id": subscription_id}))
        .map_err(|e| Error::internal(format!("serializing a revocation failed: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::num::Num;

    fn answered() -> Result<AskAnswered> {
        Ok(AskAnswered {
            schema: SchemaV1,
            ask_id: AskId::generate(),
            send_id: SendId::generate(),
            question: "Delete ~/Downloads/old.zip?".into(),
            ask_type: AskType::SingleChoice,
            answer: Answer::SingleChoice {
                option_id: "keep".into(),
                label: "Keep it".into(),
            },
            via: AnswerVia::Voice,
            transcript: Some("no keep it".into()),
            asked_at: Timestamp::parse("2026-09-26T10:00:00Z")?,
            answered_at: Timestamp::parse("2026-09-26T10:00:07Z")?,
            slot: SlotIndex::new(3)?,
            context: DataContext::Production,
        })
    }

    #[test]
    fn answered_serializes_like_the_blueprint() -> Result<()> {
        let d = answered()?;
        let v = serde_json::to_value(&d).map_err(|e| Error::internal(e.to_string()))?;
        assert_eq!(v["schema"], 1);
        assert_eq!(v["ask_type"], "single_choice");
        assert_eq!(
            v["answer"],
            json!({"kind":"single_choice","option_id":"keep","label":"Keep it"})
        );
        assert_eq!(v["via"], "voice");
        assert_eq!(v["answered_at"], "2026-09-26T10:00:07.000Z");
        assert_eq!(v["slot"], 3);
        assert_eq!(v["context"], "production");
        let back = TingData::from_value(TingType::AskAnswered, v)?;
        assert_eq!(back, TingData::AskAnswered(d));
        Ok(())
    }

    #[test]
    fn strict_parsing_of_every_type() -> Result<()> {
        let mut v =
            serde_json::to_value(answered()?).map_err(|e| Error::internal(e.to_string()))?;
        if let Some(o) = v.as_object_mut() {
            o.insert("extra".into(), json!(1));
        }
        assert!(TingData::from_value(TingType::AskAnswered, v).is_err());
        let mut v =
            serde_json::to_value(answered()?).map_err(|e| Error::internal(e.to_string()))?;
        v["schema"] = json!(2);
        assert!(TingData::from_value(TingType::AskAnswered, v).is_err());

        let msg = json!({"schema":1,"message_id":MessageId::generate(),"text":"remind me at 5","via":"keyboard",
            "sent_at":"2026-09-26T10:00:00Z","slot":1,"context":"testing","in_reply_to":null});
        assert!(TingData::from_value(TingType::MessageReceived, msg.clone()).is_ok());
        assert!(TingData::from_value(TingType::AskDismissed, msg).is_err());
        let speech = json!({"schema":1,"send_id":SendId::generate(),"stopped_by_user":false,"played_ms":4210,
            "total_ms":4210,"finished_at":"2026-09-26T10:00:00Z","slot":8,"context":"production"});
        assert!(TingData::from_value(TingType::SpeechFinished, speech).is_ok());
        let dismissed = json!({"schema":1,"send_id":SendId::generate(),"gesture":"down_arrow_double","visible_ms":2300,
            "dismissed_at":"2026-09-26T10:00:00Z","slot":2,"context":"production"});
        assert!(TingData::from_value(TingType::ShowDismissed, dismissed).is_ok());
        let expired = json!({"schema":1,"ask_id":AskId::generate(),"send_id":SendId::generate(),"question":"q","ask_type":"text",
            "asked_at":"2026-09-26T10:00:00Z","expired_at":"2026-09-26T10:01:00Z","slot":2,"context":"production"});
        assert!(TingData::from_value(TingType::AskExpired, expired).is_ok());
        let bad_slot = json!({"schema":1,"send_id":SendId::generate(),"gesture":"esc","visible_ms":1,
            "dismissed_at":"2026-09-26T10:00:00Z","slot":9,"context":"production"});
        assert!(TingData::from_value(TingType::ShowDismissed, bad_slot).is_err());
        Ok(())
    }

    #[test]
    fn semantic_checks() -> Result<()> {
        let mut d = answered()?;
        d.via = AnswerVia::Click;
        assert!(
            TingData::AskAnswered(d.clone()).validate().is_err(),
            "transcript without voice"
        );
        d.transcript = None;
        assert!(TingData::AskAnswered(d.clone()).validate().is_ok());
        d.answer = Answer::Slider {
            value: Num::from(3),
        };
        assert!(
            TingData::AskAnswered(d).validate().is_err(),
            "kind must match ask_type"
        );
        Ok(())
    }

    #[test]
    fn delivery_round_trip_and_server_validation() -> Result<()> {
        let actor = ActorId::parse("si:cleanup")?;
        let data = TingData::AskAnswered(answered()?);
        let req =
            DeliveryRequest::new(&actor, &data, TingMetadata::new(Some("deliberate".into())))?;
        assert_eq!(
            req.key,
            format!("si:cleanup/{}/answered", data.subject_id())
        );
        let bytes = req.to_bytes()?;
        let text = String::from_utf8(bytes.clone()).map_err(|e| Error::internal(e.to_string()))?;
        assert!(text.starts_with(&format!(
            "{{\"event_id\":\"{}\",\"type\":\"peek.ask.answered\",\"key\":",
            req.event_id
        )));
        let parsed: DeliveryRequest = crate::json::from_slice(&bytes, "delivery")?;
        assert_eq!(parsed, req);
        assert_eq!(parsed.validate_for(&actor)?, data);
        let other = ActorId::parse("si:intruder")?;
        assert!(
            parsed.validate_for(&other).is_err(),
            "key must start with the verified actor"
        );

        let org = OrgId::parse("tos")?;
        let body = ting_send_body(&org, &actor, &parsed)?;
        let again = ting_send_body(&org, &actor, &parsed)?;
        assert_eq!(body, again, "deterministic");
        let body = String::from_utf8(body).map_err(|e| Error::internal(e.to_string()))?;
        assert!(body.starts_with(
            r#"{"org_id":"tos","type":"peek.ask.answered","for":"si:cleanup","key":"si:cleanup/"#
        ));
        assert!(body.contains(r#""metadata":{"isi":"deliberate","peek_version":""#));

        let no_isi = serde_json::to_string(&TingMetadata::new(None))
            .map_err(|e| Error::internal(e.to_string()))?;
        assert!(!no_isi.contains("isi"));
        let unknown_meta = json!({"event_id":req.event_id,"type":"peek.ask.answered","key":req.key,"data":req.data,"metadata":{"peek_version":"0.1.0","host":"x"}});
        assert!(serde_json::from_value::<DeliveryRequest>(unknown_meta).is_err());
        let unknown_type = json!({"event_id":req.event_id,"type":"peek.other","key":req.key,"data":req.data,"metadata":{"peek_version":"0.1.0"}});
        assert!(serde_json::from_value::<DeliveryRequest>(unknown_type).is_err());
        Ok(())
    }

    #[test]
    fn enrollment_bodies_are_exact() -> Result<()> {
        let org = OrgId::parse("tos")?;
        let actor = ActorId::parse("si:cleanup")?;
        assert_eq!(
            subscription_register_body(&org, &actor)?,
            br#"{"app_id":"peek","for":"si:cleanup","org_id":"tos"}"#
        );
        assert_eq!(
            subscription_revoke_body(&org, "sub_1")?,
            br#"{"id":"sub_1","org_id":"tos"}"#
        );
        Ok(())
    }

    #[test]
    fn type_names_and_descriptions() -> Result<()> {
        for t in TingType::ALL {
            assert_eq!(TingType::parse(t.as_str())?, t);
            assert!(t.description().ends_with('.'));
            assert!(t.description().is_ascii(), "{}", t.as_str());
            let v = serde_json::to_value(t).map_err(|e| Error::internal(e.to_string()))?;
            assert_eq!(v, json!(t.as_str()));
        }
        assert!(TingType::parse("peek.other").is_err());
        assert_eq!(
            TingType::ALL.map(TingType::as_str),
            [
                "peek.ask.answered",
                "peek.ask.dismissed",
                "peek.ask.expired",
                "peek.message.received",
                "peek.speech.finished",
                "peek.show.dismissed",
                "peek.send.expired",
                "peek.schedule.due",
                "peek.send.shown",
            ]
        );
        // Descriptions are byte-identical in every context (registered once).
        assert_eq!(
            TingType::SendExpired.description(),
            "A peek --speak or --show reached its --expires-in or --expires-at deadline before it finished; the data says whether it was shown."
        );
        assert_eq!(
            TingType::ScheduleDue.description(),
            "A scheduled peek send (--in or --at) came due; the data says whether it was shown, queued, expired or replaced the active peek."
        );
        assert_eq!(
            TingType::SendShown.description(),
            "A peek send appeared on screen for the Carbon (always for scheduled sends; opt-in with --notify shown)."
        );
        assert_eq!(
            TingType::AskExpired.description(),
            "A peek --ask reached its --expires-in deadline unanswered.",
            "unchanged: re-registering a changed text breaks existing environments"
        );
        assert_eq!(TingType::SendExpired.key_event(), TingKeyEvent::SendExpired);
        assert_eq!(TingType::ScheduleDue.key_event(), TingKeyEvent::Due);
        assert_eq!(TingType::SendShown.key_event(), TingKeyEvent::Shown);
        Ok(())
    }

    fn ts(s: &str) -> Result<Timestamp> {
        Timestamp::parse(s)
    }

    fn send_expired() -> Result<SendExpired> {
        Ok(SendExpired {
            schema: SchemaV1,
            send_id: SendId::generate(),
            kind: "speak+show".into(),
            created_at: ts("2026-09-27T12:00:00Z")?,
            expires_at: ts("2026-09-27T12:10:00Z")?,
            expired_at: ts("2026-09-27T12:10:00.020Z")?,
            shown: false,
            shown_at: None,
            scheduled: false,
            schedule_id: None,
            slot: SlotIndex::new(3)?,
            context: DataContext::Production,
        })
    }

    fn schedule_due() -> Result<ScheduleDue> {
        Ok(ScheduleDue {
            schema: SchemaV1,
            schedule_id: ScheduleId::generate(),
            send_id: SendId::generate(),
            ask_id: None,
            kind: "show".into(),
            due_at: ts("2026-09-27T12:30:00Z")?,
            fired_at: ts("2026-09-27T12:30:00.012Z")?,
            outcome: DueOutcome::Queued,
            replaced_send_id: None,
            waiting_reason: Some(WaitingReason::BehindOthers),
            queue_position: Some(2),
            expires_at: None,
            slot: SlotIndex::new(3)?,
            context: DataContext::Production,
        })
    }

    fn send_shown() -> Result<SendShown> {
        Ok(SendShown {
            schema: SchemaV1,
            send_id: SendId::generate(),
            ask_id: Some(AskId::generate()),
            kind: "speak+ask".into(),
            created_at: ts("2026-09-27T12:00:00Z")?,
            shown_at: ts("2026-09-27T12:30:00.300Z")?,
            scheduled: true,
            schedule_id: Some(ScheduleId::generate()),
            slot: SlotIndex::new(3)?,
            context: DataContext::Testing,
        })
    }

    fn to_v<T: Serialize>(t: &T) -> Result<Value> {
        serde_json::to_value(t).map_err(|e| Error::internal(e.to_string()))
    }

    #[test]
    fn new_types_are_strict() -> Result<()> {
        let cases = [
            (TingType::SendExpired, to_v(&send_expired()?)?),
            (TingType::ScheduleDue, to_v(&schedule_due()?)?),
            (TingType::SendShown, to_v(&send_shown()?)?),
        ];
        for (t, v) in cases {
            let parsed = TingData::from_value(t, v.clone())?;
            parsed.validate()?;
            assert_eq!(parsed.ting_type(), t);
            assert_eq!(to_v(&parsed)?, v, "{}", t.as_str());
            let mut extra = v.clone();
            extra["extra"] = json!(1);
            assert!(
                TingData::from_value(t, extra).is_err(),
                "{}: unknown field",
                t.as_str()
            );
            let mut schema2 = v.clone();
            schema2["schema"] = json!(2);
            assert!(
                TingData::from_value(t, schema2).is_err(),
                "{}: schema 2",
                t.as_str()
            );
            let mut slot9 = v.clone();
            slot9["slot"] = json!(9);
            assert!(
                TingData::from_value(t, slot9).is_err(),
                "{}: slot 9",
                t.as_str()
            );
            let mut missing = v.clone();
            if let Some(o) = missing.as_object_mut() {
                o.remove("context");
            }
            assert!(
                TingData::from_value(t, missing).is_err(),
                "{}: context required",
                t.as_str()
            );
        }
        // Nullable fields are always present on the wire.
        let v = to_v(&send_expired()?)?;
        assert!(v["shown_at"].is_null() && v["schedule_id"].is_null());
        let v = to_v(&schedule_due()?)?;
        assert_eq!(v["outcome"], "queued");
        assert_eq!(v["waiting_reason"], "behind_others");
        assert!(
            v["replaced_send_id"].is_null() && v["expires_at"].is_null() && v["ask_id"].is_null()
        );
        assert_eq!(v["fired_at"], "2026-09-27T12:30:00.012Z");
        Ok(())
    }

    #[test]
    fn new_types_semantic_checks() -> Result<()> {
        let bad = |d: TingData| d.validate().is_err();
        // peek.send.expired
        let mut d = send_expired()?;
        d.kind = "ask".into();
        assert!(bad(TingData::SendExpired(d)));
        for kind in ["speak", "show", "speak+show"] {
            let mut d = send_expired()?;
            d.kind = kind.into();
            assert!(!bad(TingData::SendExpired(d)), "{kind}");
        }
        let mut d = send_expired()?;
        d.shown = true;
        assert!(
            bad(TingData::SendExpired(d.clone())),
            "shown without shown_at"
        );
        d.shown_at = Some(ts("2026-09-27T12:00:01Z")?);
        assert!(!bad(TingData::SendExpired(d.clone())));
        d.shown = false;
        assert!(bad(TingData::SendExpired(d)), "shown_at without shown");
        let mut d = send_expired()?;
        d.scheduled = true;
        assert!(bad(TingData::SendExpired(d.clone())));
        d.schedule_id = Some(ScheduleId::generate());
        assert!(!bad(TingData::SendExpired(d.clone())));
        d.scheduled = false;
        assert!(bad(TingData::SendExpired(d)));
        let mut d = send_expired()?;
        d.expired_at = ts("2026-09-27T11:59:59Z")?;
        assert!(bad(TingData::SendExpired(d)));
        // peek.schedule.due
        let mut d = schedule_due()?;
        d.kind = "speak+show+ask".into();
        assert!(bad(TingData::ScheduleDue(d)));
        let mut d = schedule_due()?;
        d.outcome = DueOutcome::Replaced;
        assert!(
            bad(TingData::ScheduleDue(d.clone())),
            "replaced needs replaced_send_id"
        );
        d.replaced_send_id = Some(SendId::generate());
        d.queue_position = None;
        assert!(
            !bad(TingData::ScheduleDue(d.clone())),
            "replaced but held keeps a waiting_reason"
        );
        d.waiting_reason = None;
        assert!(!bad(TingData::ScheduleDue(d.clone())));
        d.outcome = DueOutcome::Shown;
        assert!(
            bad(TingData::ScheduleDue(d.clone())),
            "replaced_send_id only with replaced"
        );
        d.replaced_send_id = None;
        assert!(!bad(TingData::ScheduleDue(d.clone())));
        d.waiting_reason = Some(WaitingReason::Paused);
        assert!(
            bad(TingData::ScheduleDue(d.clone())),
            "shown has no waiting_reason"
        );
        d.waiting_reason = None;
        d.queue_position = Some(0);
        assert!(
            bad(TingData::ScheduleDue(d.clone())),
            "queue_position only with queued"
        );
        d.queue_position = None;
        d.outcome = DueOutcome::Expired;
        assert!(!bad(TingData::ScheduleDue(d.clone())));
        d.waiting_reason = Some(WaitingReason::CarbonAway);
        assert!(
            bad(TingData::ScheduleDue(d.clone())),
            "expired has no waiting_reason"
        );
        let mut d = schedule_due()?;
        d.waiting_reason = None;
        assert!(
            bad(TingData::ScheduleDue(d.clone())),
            "queued needs a waiting_reason"
        );
        d.waiting_reason = Some(WaitingReason::QueueFull);
        d.queue_position = None;
        assert!(bad(TingData::ScheduleDue(d)), "queued needs queue_position");
        let mut d = schedule_due()?;
        d.fired_at = ts("2026-09-27T12:29:59Z")?;
        assert!(bad(TingData::ScheduleDue(d)));
        // peek.send.shown
        let mut d = send_shown()?;
        d.schedule_id = None;
        assert!(bad(TingData::SendShown(d.clone())));
        d.scheduled = false;
        assert!(!bad(TingData::SendShown(d.clone())));
        d.shown_at = ts("2026-09-27T11:00:00Z")?;
        assert!(bad(TingData::SendShown(d)));
        let mut d = send_shown()?;
        d.kind = "speaking".into();
        assert!(bad(TingData::SendShown(d)));
        Ok(())
    }

    #[test]
    fn new_types_deliver_with_their_keys() -> Result<()> {
        let actor = ActorId::parse("si:cleanup")?;
        let org = OrgId::parse("tos")?;
        let expired = send_expired()?;
        let due = schedule_due()?;
        let shown = send_shown()?;
        let cases = [
            (
                TingData::SendExpired(expired.clone()),
                format!("si:cleanup/{}/send_expired", expired.send_id),
                "peek.send.expired",
            ),
            (
                TingData::ScheduleDue(due.clone()),
                format!("si:cleanup/{}/due", due.schedule_id),
                "peek.schedule.due",
            ),
            (
                TingData::SendShown(shown.clone()),
                format!("si:cleanup/{}/shown", shown.send_id),
                "peek.send.shown",
            ),
        ];
        for (data, key, name) in cases {
            let req = DeliveryRequest::new(&actor, &data, TingMetadata::new(None))?;
            assert_eq!(req.key, key);
            let bytes = req.to_bytes()?;
            let parsed: DeliveryRequest = crate::json::from_slice(&bytes, "delivery")?;
            assert_eq!(parsed.validate_for(&actor)?, data);
            let body = String::from_utf8(ting_send_body(&org, &actor, &parsed)?)
                .map_err(|e| Error::internal(e.to_string()))?;
            assert!(body.starts_with(&format!(
                r#"{{"org_id":"tos","type":"{name}","for":"si:cleanup","key":"{key}""#
            )));
            let mut wrong = parsed.clone();
            wrong.key = format!("si:other/{}", key.split_once('/').map_or("", |k| k.1));
            assert!(wrong.validate_for(&actor).is_err());
        }
        // A delivery with invalid semantics is refused at construction.
        let mut e = send_expired()?;
        e.shown = true;
        assert!(
            DeliveryRequest::new(&actor, &TingData::SendExpired(e), TingMetadata::new(None))
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn ask_expired_with_and_without_shown_and_esc_double() -> Result<()> {
        let base = json!({"schema":1,"ask_id":AskId::generate(),"send_id":SendId::generate(),"question":"q",
            "ask_type":"text","asked_at":"2026-09-26T10:00:00.000Z","expired_at":"2026-09-26T10:01:00.000Z",
            "slot":2,"context":"production"});
        let old = TingData::from_value(TingType::AskExpired, base.clone())?;
        let TingData::AskExpired(old) = old else {
            return Err(Error::internal("wrong variant"));
        };
        assert_eq!(old.shown, None);
        assert!(to_v(&old)?.get("shown").is_none(), "absent stays absent");
        let mut with = base;
        with["shown"] = json!(false);
        let new = TingData::from_value(TingType::AskExpired, with.clone())?;
        assert_eq!(to_v(&new)?, with);
        let esc = json!({"schema":1,"send_id":SendId::generate(),"gesture":"esc_double","visible_ms":900,
            "dismissed_at":"2026-09-26T10:00:00Z","slot":2,"context":"production"});
        let d = TingData::from_value(TingType::ShowDismissed, esc)?;
        let TingData::ShowDismissed(d) = d else {
            return Err(Error::internal("wrong variant"));
        };
        assert_eq!(d.gesture, Gesture::EscDouble);
        let dismissed = json!({"schema":1,"ask_id":AskId::generate(),"send_id":SendId::generate(),"question":"q",
            "ask_type":"text","gesture":"esc_double","asked_at":"2026-09-26T10:00:00Z",
            "dismissed_at":"2026-09-26T10:01:00Z","slot":2,"context":"production"});
        assert!(TingData::from_value(TingType::AskDismissed, dismissed).is_ok());
        Ok(())
    }
}
