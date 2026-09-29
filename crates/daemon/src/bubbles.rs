//! Sends, asks and the per-Silicon queue (BLUEPRINT §1.9.3–§1.9.7, §7.4, as
//! changed by the 0.1.2 contract §6).
//!
//! Queueing (always queue, strict FIFO): a new send never replaces what is on
//! screen. It becomes the current bubble when there is none, else it waits
//! behind the current one, at most five waiting, then `queue_full` (legacy
//! CLIs get `slot_busy`). Only `--replace` takes over the current bubble. Due
//! scheduled sends that find five waiting wait in an overflow list and take
//! the next free spot before any new send. The same Silicon's concurrent
//! ISIs share one queue. Every transition runs under [`Shared::core`], and
//! the database is the source of truth, so queues survive a restart.
//!
//! Every send may expire (`--expires-in`/`--expires-at`): a waiting one is
//! dropped unseen, an on-screen one slides away, and the Silicon gets
//! `peek.ask.expired` or `peek.send.expired` saying whether it was shown.
//!
//! Each answer reaches the Silicon through exactly one channel (D22): a live
//! `send --wait` connection, or a ting through the outbox.

use std::{
    collections::VecDeque,
    sync::Arc,
    time::{Duration, Instant},
};

use rusqlite::{Connection, OptionalExtension as _, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use silicon_peek_client::{
    Error, ErrorCode, Result,
    identity::{ActorId, ApiUrl, Context, OrgId, SlotIndex},
    ids::{AskId, MessageId, ScheduleId, SendId},
    ipc::{
        cli::{
            AskInfo, AskResult, AskState, DeliveryInfo, DeliveryState, HeldReason, SendOp,
            SendResult, SendStatus, SpeechInfo, SpeechStatus, Warning, warnings,
        },
        ui::{
            AnswerOp, CancelReason, Dismissed, MessageOp, MessageResult, PeekCancel, PeekShow,
            PresenceReason, Shown, ShownDone, SpeakInfo, SpeechDone,
        },
    },
    schema::{
        ImageFormat, ImageHop, ImageRef,
        ask::{Answer, Ask},
        limits,
        send::{Notify, check_voice, check_voice_instructions},
        show::Show,
    },
    timestamp::Timestamp,
    ting::{
        AnswerVia, AskAnswered, AskDismissed, AskExpired, Gesture, MessageReceived, MessageVia,
        SchemaV1, SendShown, ShowDismissed, SpeechFinished, TingData,
    },
};
use tokio::sync::oneshot;

use crate::{
    db::SqlResult as _,
    net::HomeRef,
    outbox,
    paths::sha256_hex,
    queue::{CancelledAll, Withdraw, cancelled_result},
    speech::{TtsCache, TtsJob},
    state::{ActorKey, Bubble, Caller, Core, Shared, SharedRef, WaiterMsg},
    telemetry::Record,
    voice,
};

/// The first Peek.app build that reports `shown` (0.1.2 = 1002); older apps
/// get 0.1.1's push-time `shown_at` and speech start.
pub const SHOWN_OP_BUILD: u64 = 1002;

/// Current unix milliseconds.
#[must_use]
pub fn now_ms() -> i64 {
    Timestamp::now().unix_ms()
}

/// A send's speech as stored: the plan reported to the CLI, then its outcome
/// once the UI finished (`played`) or TTS failed before any audio (`failed`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoredSpeechStatus {
    /// Planned; streaming from Gemini when shown.
    Pending,
    /// Served from the local TTS cache (kept once played).
    Cached,
    /// No speech.
    Skipped,
    /// Legacy status for speech unsupported by an older provider.
    UnsupportedLanguage,
    /// Streamed from Gemini and played.
    Played,
    /// TTS failed before any audio played.
    Failed,
}

impl From<SpeechStatus> for StoredSpeechStatus {
    fn from(s: SpeechStatus) -> Self {
        match s {
            SpeechStatus::Pending => Self::Pending,
            SpeechStatus::Cached => Self::Cached,
            SpeechStatus::Skipped => Self::Skipped,
            SpeechStatus::UnsupportedLanguage => Self::UnsupportedLanguage,
        }
    }
}

impl StoredSpeechStatus {
    /// What `peek.show` tells the UI: a finished or failed speech is never
    /// spoken again (a re-shown ask shows its text instead).
    #[must_use]
    pub const fn for_ui(self) -> SpeechStatus {
        match self {
            Self::Pending => SpeechStatus::Pending,
            Self::Cached => SpeechStatus::Cached,
            Self::UnsupportedLanguage => SpeechStatus::UnsupportedLanguage,
            Self::Skipped | Self::Played | Self::Failed => SpeechStatus::Skipped,
        }
    }

    /// Whether TTS still has to run.
    const fn to_speak(self) -> bool {
        matches!(self, Self::Pending | Self::Cached)
    }
}

/// Records how a send's speech ended in `sends.payload` (`played` for a
/// streamed voice, `cached` stays `cached`, `failed`); a final status is
/// never overwritten.
///
/// # Errors
/// Database failures.
pub fn set_speech_outcome(
    c: &Connection,
    send_id: &str,
    outcome: StoredSpeechStatus,
) -> Result<()> {
    let payload: Option<Vec<u8>> = c
        .query_row(
            "SELECT payload FROM sends WHERE send_id = ?1",
            [send_id],
            |r| r.get(0),
        )
        .optional()
        .sql()?;
    let Some(mut payload) = payload.and_then(|p| serde_json::from_slice::<SendPayload>(&p).ok())
    else {
        return Ok(());
    };
    let Some(speech) = payload.speech.as_mut() else {
        return Ok(());
    };
    let next = match (speech.status, outcome) {
        (StoredSpeechStatus::Pending, o) => o,
        (StoredSpeechStatus::Cached, StoredSpeechStatus::Failed) => StoredSpeechStatus::Failed,
        _ => return Ok(()),
    };
    speech.status = next;
    let bytes = serde_json::to_vec(&payload)
        .map_err(|e| Error::internal(format!("serializing a send failed: {e}")))?;
    c.execute(
        "UPDATE sends SET payload = ?2 WHERE send_id = ?1",
        params![send_id, bytes],
    )
    .sql()?;
    Ok(())
}

/// How TTS was planned for a send, and how it ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredSpeech {
    /// The plan reported to the CLI, then the outcome.
    pub status: StoredSpeechStatus,
    /// The voice.
    #[serde(default)]
    pub model: Option<String>,
    /// The language.
    #[serde(default)]
    pub language: Option<String>,
    /// Delivery instructions captured at enqueue time.
    #[serde(default)]
    pub voice_instructions: Option<String>,
}

/// What a bubble shows, stored in `sends.payload`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SendPayload {
    /// `--speak`.
    #[serde(default)]
    pub speak: Option<String>,
    /// The TTS plan.
    #[serde(default)]
    pub speech: Option<StoredSpeech>,
    /// `--show`, with cache paths.
    #[serde(default)]
    pub show: Option<Show>,
    /// `--ask`, with cache paths.
    #[serde(default)]
    pub ask: Option<Ask>,
    /// How long the show (or pill) stays up.
    #[serde(default)]
    pub duration_ms: Option<u64>,
    /// When the send expires (unix ms), for every kind (0.1.1: asks only).
    #[serde(default)]
    pub expires_at: Option<i64>,
}

impl SendPayload {
    /// The one-line summary `peek queue` and `peek schedule list` show
    /// (contract §6.9): the question of an ask; a show's first text, else its
    /// first image caption, else `image`; a speak-only send's text.
    /// Whitespace collapses to single spaces; at most 60 characters, ending
    /// with `…` when cut.
    #[must_use]
    pub fn summary(&self) -> String {
        use silicon_peek_client::schema::show::ShowElement;
        let raw = if let Some(a) = &self.ask {
            a.question.clone()
        } else if let Some(s) = &self.show {
            s.elements
                .iter()
                .find_map(|e| match e {
                    ShowElement::Text { text } => Some(text.clone()),
                    ShowElement::Image { .. } => None,
                })
                .or_else(|| {
                    s.elements.iter().find_map(|e| match e {
                        ShowElement::Image {
                            caption: Some(c), ..
                        } if !c.trim().is_empty() => Some(c.clone()),
                        _ => None,
                    })
                })
                .unwrap_or_else(|| "image".to_owned())
        } else {
            self.speak.clone().unwrap_or_default()
        };
        summarize(&raw)
    }
}

/// Collapses whitespace and cuts to [`limits::SUMMARY_MAX_CHARS`] characters.
#[must_use]
pub fn summarize(raw: &str) -> String {
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let max = limits::SUMMARY_MAX_CHARS;
    if collapsed.chars().count() <= max {
        collapsed
    } else {
        let mut cut: String = collapsed.chars().take(max - 1).collect();
        cut.push('…');
        cut
    }
}

/// A `sends` row.
#[derive(Clone, Debug)]
pub struct SendRow {
    /// The send.
    pub send_id: SendId,
    /// Its Silicon.
    pub key: ActorKey,
    /// The slot at send time.
    pub slot: SlotIndex,
    /// `$ISI`.
    pub isi: Option<String>,
    /// The bubble.
    pub payload: SendPayload,
    /// Opt-in notifications.
    pub notify: Vec<Notify>,
    /// Unix ms.
    pub created_at: i64,
    /// Unix ms.
    pub shown_at: Option<i64>,
    /// Unix ms.
    pub closed_at: Option<i64>,
    /// Home and backend.
    pub home: HomeRef,
    /// Unix ms.
    pub speech_done_at: Option<i64>,
    /// `speak`, `show`, `ask`, `speak+show` or `speak+ask`.
    pub kind: String,
    /// Unix ms (every kind; schema 2).
    pub expires_at: Option<i64>,
    /// Unix ms it entered its queue.
    pub queued_at: i64,
    /// A due scheduled send waiting for a free spot.
    pub overflow: bool,
    /// The scheduled send it came from.
    pub schedule_id: Option<ScheduleId>,
    /// When it was due (unix ms), for a scheduled send.
    pub due_at: Option<i64>,
    /// Why it closed.
    pub close_reason: Option<String>,
}

/// An `asks` row.
#[derive(Clone, Debug)]
pub struct AskRow {
    /// The ask.
    pub ask_id: AskId,
    /// Its send.
    pub send_id: SendId,
    /// State.
    pub state: AskState,
    /// The typed answer.
    pub answer: Option<Answer>,
    /// How.
    pub via: Option<AnswerVia>,
    /// Voice transcript.
    pub transcript: Option<String>,
    /// Unix ms.
    pub answered_at: Option<i64>,
    /// Unix ms.
    pub expires_at: Option<i64>,
    /// `ting` or `wait`.
    pub delivered_via: Option<String>,
    /// The outbox row.
    pub event_id: Option<String>,
    /// Unix ms.
    pub created_at: i64,
}

fn enum_str<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn parse_enum<T: for<'de> Deserialize<'de>>(s: &str) -> Option<T> {
    serde_json::from_value(Value::String(s.to_owned())).ok()
}

fn corrupt(what: &str, e: impl std::fmt::Display) -> Error {
    Error::new(
        ErrorCode::StoreCorrupt,
        format!("peekd.sqlite holds an unreadable {what}: {e}"),
    )
    .with_hint("move ~/Library/Application Support/Peek/peekd.sqlite aside and reopen Peek")
}

const SEND_COLS: &str = "send_id, context, org_id, actor_id, slot, isi, payload, notify, created_at, shown_at, closed_at, home_path, api_url, speech_done_at, kind, expires_at, queued_at, overflow, schedule_id, due_at, close_reason";

fn send_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Result<SendRow>> {
    let send_id: String = r.get(0)?;
    let context: String = r.get(1)?;
    let org: String = r.get(2)?;
    let actor: String = r.get(3)?;
    let slot: i64 = r.get(4)?;
    let isi: Option<String> = r.get(5)?;
    let payload: Vec<u8> = r.get(6)?;
    let notify: String = r.get(7)?;
    let created_at: i64 = r.get(8)?;
    let shown_at: Option<i64> = r.get(9)?;
    let closed_at: Option<i64> = r.get(10)?;
    let home_path: String = r.get(11)?;
    let api_url: String = r.get(12)?;
    let speech_done_at: Option<i64> = r.get(13)?;
    let kind: String = r.get(14)?;
    let expires_at: Option<i64> = r.get(15)?;
    let queued_at: Option<i64> = r.get(16)?;
    let overflow: i64 = r.get(17)?;
    let schedule_id: Option<String> = r.get(18)?;
    let due_at: Option<i64> = r.get(19)?;
    let close_reason: Option<String> = r.get(20)?;
    Ok((|| {
        let context = Context::parse(&context).map_err(|e| corrupt("send context", e))?;
        Ok(SendRow {
            send_id: SendId::parse(&send_id).map_err(|e| corrupt("send id", e))?,
            key: ActorKey {
                context,
                org: OrgId::parse(&org).map_err(|e| corrupt("org", e))?,
                actor: ActorId::parse(&actor).map_err(|e| corrupt("actor", e))?,
            },
            slot: SlotIndex::new(u64::try_from(slot).unwrap_or(0))
                .map_err(|e| corrupt("slot", e))?,
            isi,
            payload: serde_json::from_slice(&payload).map_err(|e| corrupt("send payload", e))?,
            notify: serde_json::from_str(&notify).map_err(|e| corrupt("notify list", e))?,
            created_at,
            shown_at,
            closed_at,
            home: HomeRef {
                home_path,
                api_url: ApiUrl::parse(&api_url).map_err(|e| corrupt("api url", e))?,
                context,
            },
            speech_done_at,
            kind,
            expires_at,
            queued_at: queued_at.unwrap_or(created_at),
            overflow: overflow != 0,
            schedule_id: schedule_id
                .map(|s| ScheduleId::parse(&s))
                .transpose()
                .map_err(|e| corrupt("schedule id", e))?,
            due_at,
            close_reason,
        })
    })())
}

/// Loads a send.
///
/// # Errors
/// Database failures; `store_corrupt`.
pub fn load_send(c: &Connection, send_id: &str) -> Result<Option<SendRow>> {
    let row = c
        .query_row(
            &format!("SELECT {SEND_COLS} FROM sends WHERE send_id = ?1"),
            [send_id],
            send_from_row,
        )
        .optional()
        .sql()?;
    row.transpose()
}

const ASK_COLS: &str = "ask_id, send_id, state, answer, via, transcript, answered_at, expires_at, delivered_via, event_id, created_at";

fn ask_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Result<AskRow>> {
    let ask_id: String = r.get(0)?;
    let send_id: String = r.get(1)?;
    let state: String = r.get(2)?;
    let answer: Option<Vec<u8>> = r.get(3)?;
    let via: Option<String> = r.get(4)?;
    let transcript: Option<String> = r.get(5)?;
    let answered_at: Option<i64> = r.get(6)?;
    let expires_at: Option<i64> = r.get(7)?;
    let delivered_via: Option<String> = r.get(8)?;
    let event_id: Option<String> = r.get(9)?;
    let created_at: i64 = r.get(10)?;
    Ok((|| {
        Ok(AskRow {
            ask_id: AskId::parse(&ask_id).map_err(|e| corrupt("ask id", e))?,
            send_id: SendId::parse(&send_id).map_err(|e| corrupt("send id", e))?,
            state: parse_enum(&state).ok_or_else(|| corrupt("ask state", &state))?,
            answer: answer
                .map(|b| serde_json::from_slice(&b))
                .transpose()
                .map_err(|e| corrupt("answer", e))?,
            via: via.as_deref().and_then(parse_enum),
            transcript,
            answered_at,
            expires_at,
            delivered_via,
            event_id,
            created_at,
        })
    })())
}

/// Loads an ask.
///
/// # Errors
/// Database failures; `store_corrupt`.
pub fn load_ask(c: &Connection, ask_id: &str) -> Result<Option<AskRow>> {
    c.query_row(
        &format!("SELECT {ASK_COLS} FROM asks WHERE ask_id = ?1"),
        [ask_id],
        ask_from_row,
    )
    .optional()
    .sql()?
    .transpose()
}

/// The ask of a send, if any.
///
/// # Errors
/// Database failures.
pub fn ask_of_send(c: &Connection, send_id: &str) -> Result<Option<AskRow>> {
    c.query_row(
        &format!("SELECT {ASK_COLS} FROM asks WHERE send_id = ?1"),
        [send_id],
        ask_from_row,
    )
    .optional()
    .sql()?
    .transpose()
}

/// Adds a `{code,message}` note to a send's history row.
///
/// # Errors
/// Database failures.
pub fn append_send_warning(c: &Connection, send_id: &str, warning: Value) -> Result<()> {
    let existing: Option<Option<String>> = c
        .query_row(
            "SELECT warnings FROM sends WHERE send_id = ?1",
            [send_id],
            |r| r.get(0),
        )
        .optional()
        .sql()?;
    let Some(existing) = existing else {
        return Ok(());
    };
    let mut list: Vec<Value> = existing
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    list.push(warning);
    c.execute(
        "UPDATE sends SET warnings = ?2 WHERE send_id = ?1",
        params![send_id, Value::Array(list).to_string()],
    )
    .sql()?;
    Ok(())
}

/// Puts the warnings a send returned to the CLI first in its history row,
/// ahead of any added since (a speech failure can race the send's reply).
///
/// # Errors
/// Database failures.
pub fn prepend_send_warnings(c: &Connection, send_id: &str, warnings: Vec<Value>) -> Result<()> {
    let existing: Option<Option<String>> = c
        .query_row(
            "SELECT warnings FROM sends WHERE send_id = ?1",
            [send_id],
            |r| r.get(0),
        )
        .optional()
        .sql()?;
    let Some(existing) = existing else {
        return Ok(());
    };
    let mut list = warnings;
    list.extend(
        existing
            .and_then(|s| serde_json::from_str::<Vec<Value>>(&s).ok())
            .unwrap_or_default(),
    );
    c.execute(
        "UPDATE sends SET warnings = ?2 WHERE send_id = ?1",
        params![send_id, Value::Array(list).to_string()],
    )
    .sql()?;
    Ok(())
}

/// `carbon_away`: the send waits until the Carbon can see it.
pub(crate) fn carbon_away_warning(reason: PresenceReason, slot: SlotIndex) -> Warning {
    let why = match reason {
        PresenceReason::Asleep | PresenceReason::DisplayOff => "their display is asleep",
        _ => "their screen is locked",
    };
    Warning {
        code: warnings::CARBON_AWAY.to_owned(),
        message: format!(
            "the Carbon cannot see bubbles right now ({why}); this send waits in position {slot}'s queue and is shown, and spoken, when they are back"
        ),
        details: Some(json!({"reason": reason})),
    }
}

/// `carbon_paused`: the Carbon paused Peek; the send waits until they resume.
pub(crate) fn carbon_paused_warning(slot: SlotIndex) -> Warning {
    Warning {
        code: warnings::CARBON_PAUSED.to_owned(),
        message: format!(
            "Peek is paused by the Carbon; this send waits in position {slot}'s queue and is shown when they resume"
        ),
        details: None,
    }
}

/// The Silicon's current slot.
///
/// # Errors
/// Database failures.
pub fn slot_of(c: &Connection, key: &ActorKey) -> Result<Option<SlotIndex>> {
    let slot: Option<i64> = c
        .query_row(
            "SELECT slot FROM slots WHERE context = ?1 AND org_id = ?2 AND actor_id = ?3",
            params![key.context_str(), key.org.as_str(), key.actor.as_str()],
            |r| r.get(0),
        )
        .optional()
        .sql()?;
    Ok(slot.and_then(|s| SlotIndex::new(u64::try_from(s).unwrap_or(0)).ok()))
}

/// Free slots of a context, for `side_taken` / `side_not_registered`.
///
/// # Errors
/// Database failures.
pub fn free_slots(c: &Connection, context: Context) -> Result<Vec<u8>> {
    let mut st = c
        .prepare("SELECT slot FROM slots WHERE context = ?1")
        .sql()?;
    let taken: Vec<i64> = st
        .query_map([context.as_string()], |r| r.get(0))
        .sql()?
        .collect::<rusqlite::Result<_>>()
        .sql()?;
    Ok(SlotIndex::ALL
        .iter()
        .map(|s| s.get())
        .filter(|s| !taken.contains(&i64::from(*s)))
        .collect())
}

fn free_list(free: &[u8]) -> String {
    if free.is_empty() {
        "none".to_owned()
    } else {
        free.iter().map(u8::to_string).collect::<Vec<_>>().join(",")
    }
}

/// `side_not_registered` with the exact §7.5 message.
#[must_use]
pub fn side_not_registered(actor: &ActorId, free: &[u8]) -> Error {
    Error::new(
        ErrorCode::SideNotRegistered,
        format!(
            "no position registered for {actor}; run `peek register side <1-8>` first (free: {})",
            free_list(free)
        ),
    )
    .with_hint(match free.first() {
        Some(f) => format!("peek register side {f}"),
        None => {
            "every position is taken; ask a Carbon to free one (peek unregister from that Silicon)"
                .to_owned()
        }
    })
    .with_details(json!({"free": free}))
}

/// `queue_full` (contract §6.3): one send on screen and five waiting. The
/// connection of a CLI older than 0.1.2 relabels it `slot_busy`.
pub(crate) fn queue_full(
    slot: SlotIndex,
    queue: &crate::state::ActorQueue,
    held: Option<HeldReason>,
) -> Error {
    let queued = queue.waiting.len();
    let suffix = match held {
        Some(HeldReason::CarbonAway) => {
            " (the Carbon's screen is locked or asleep, so nothing moves until they are back)"
        }
        Some(HeldReason::Paused) => " (the Carbon paused Peek, so nothing moves until they resume)",
        _ => "",
    };
    Error::new(
        ErrorCode::QueueFull,
        format!(
            "position {slot}'s queue is full: 1 send on screen and {queued} waiting (at most {}); remove one with `peek cancel <send_id>` or `peek queue clear`{suffix}",
            limits::QUEUE_MAX
        ),
    )
    .with_hint("peek queue    lists the waiting sends and their IDs")
    .with_retryable(true)
    .with_details(json!({
        "queued": queued,
        "limit": limits::QUEUE_MAX,
        "on_screen": queue.current.as_ref().map(|b| b.send_id.as_str()),
        "waiting": queue.waiting.iter().map(|b| b.send_id.as_str()).collect::<Vec<_>>(),
        "due_waiting": queue.overflow.len(),
        "held": held,
    }))
}

fn pill_duration(chars: usize) -> u64 {
    let chars = u64::try_from(chars).unwrap_or(u64::MAX);
    chars
        .saturating_mul(60)
        .saturating_add(3000)
        .clamp(4000, 15_000)
}

/// The `peek.show` event for a send.
fn peek_show_event(
    send: &SendRow,
    bubble: &Bubble,
    slot: SlotIndex,
    queued_behind: u32,
    key: &ActorKey,
) -> PeekShow {
    PeekShow {
        send_id: send.send_id.clone(),
        slot,
        context: key.context,
        speak: send.payload.speak.as_ref().map(|t| SpeakInfo {
            text: t.clone(),
            status: send
                .payload
                .speech
                .as_ref()
                .map_or(SpeechStatus::Skipped, |s| s.status.for_ui()),
        }),
        show: send.payload.show.clone(),
        ask: send.payload.ask.clone(),
        ask_id: bubble.ask_id.clone(),
        duration_ms: send.payload.duration_ms,
        queued_behind,
        expires_at: send
            .expires_at
            .or(send.payload.expires_at)
            .map(Timestamp::from_unix_ms),
        replaces: bubble.replaces.clone(),
        schedule_id: send.schedule_id.clone(),
    }
}

/// A send (and its ask) to store.
pub(crate) struct NewSend {
    pub(crate) send_id: String,
    pub(crate) ask_id: Option<String>,
    pub(crate) key: ActorKey,
    pub(crate) home: HomeRef,
    pub(crate) slot: SlotIndex,
    pub(crate) isi: Option<String>,
    pub(crate) payload: Vec<u8>,
    pub(crate) notify: String,
    pub(crate) kind: String,
    pub(crate) expires_at: Option<i64>,
    pub(crate) waiter: i64,
    /// When `peek send` ran.
    pub(crate) created_at: i64,
    /// When it entered its queue (now; the fire time for a scheduled send).
    pub(crate) queued_at: i64,
    pub(crate) schedule_id: Option<String>,
    pub(crate) due_at: Option<i64>,
    pub(crate) warnings: Option<String>,
}

pub(crate) fn insert_send(tx: &Connection, row: &NewSend) -> Result<()> {
    tx.execute(
        "INSERT INTO sends (send_id, context, org_id, actor_id, slot, isi, payload, notify, created_at, home_path, api_url, kind,
                            expires_at, queued_at, schedule_id, due_at, warnings)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
        params![
            row.send_id,
            row.key.context_str(),
            row.key.org.as_str(),
            row.key.actor.as_str(),
            row.slot.get(),
            row.isi,
            row.payload,
            row.notify,
            row.created_at,
            row.home.home_path,
            row.home.api_url.as_str(),
            row.kind,
            row.expires_at,
            row.queued_at,
            row.schedule_id,
            row.due_at,
            row.warnings
        ],
    )
    .sql()?;
    if let Some(aid) = &row.ask_id {
        tx.execute(
            "INSERT INTO asks (ask_id, send_id, state, expires_at, created_at, waiter) VALUES (?1, ?2, 'pending', ?3, ?4, ?5)",
            params![aid, row.send_id, row.expires_at, row.created_at, row.waiter],
        )
        .sql()?;
    }
    Ok(())
}

/// Where a send was taken from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Slotted {
    /// The current bubble; whether `peek.show` reached the UI.
    Current {
        /// Whether `peek.show` reached the UI.
        pushed: bool,
    },
    /// Waiting, at this 0-based index.
    Waiting(usize),
    /// A due scheduled send waiting for room, at this 0-based index.
    Overflow(usize),
}

/// Removes a send from its queue (current, waiting or overflow) and says
/// where it was.
pub(crate) fn take_from_queue(
    core: &mut Core,
    key: &ActorKey,
    send_id: &SendId,
) -> Option<Slotted> {
    let q = core.queues.get_mut(key)?;
    if q.current.as_ref().is_some_and(|b| &b.send_id == send_id) {
        let pushed = q.current.take().is_some_and(|b| b.pushed);
        return Some(Slotted::Current { pushed });
    }
    if let Some(i) = q.waiting.iter().position(|b| &b.send_id == send_id) {
        q.waiting.remove(i);
        return Some(Slotted::Waiting(i));
    }
    if let Some(i) = q.overflow.iter().position(|b| &b.send_id == send_id) {
        q.overflow.remove(i);
        return Some(Slotted::Overflow(i));
    }
    None
}

fn op_wait(op: &SendOp) -> i64 {
    i64::from(op.wait && op.ask.is_some())
}

/// What the bubble shows: duration defaults (§7.4) and the send's expiry
/// (every kind): `--expires-in` from `now` (peekd's receipt), or the
/// absolute `--expires-at`.
pub(crate) fn build_payload(
    op: &SendOp,
    show: Option<Show>,
    ask: Option<Ask>,
    speech: Option<StoredSpeech>,
    now: i64,
) -> SendPayload {
    let duration_ms = op.duration_ms.or_else(|| {
        show.as_ref()
            .map(|s| u64::try_from(s.default_duration().as_millis()).unwrap_or(15_000))
            .or_else(|| op.speak.as_ref().map(|t| pill_duration(t.chars().count())))
    });
    let expires_at = op
        .expires_in_s
        .map(|s| now.saturating_add(i64::try_from(s.saturating_mul(1000)).unwrap_or(i64::MAX)))
        .or_else(|| op.expires_at.map(Timestamp::unix_ms));
    let is_ask = ask.is_some();
    SendPayload {
        speak: op.speak.clone(),
        speech,
        show,
        ask,
        duration_ms: if is_ask { None } else { duration_ms },
        expires_at,
    }
}

/// Why an ask ends.
#[derive(Clone, Debug)]
pub enum Resolution {
    /// Answered by click, keyboard or voice.
    Answered {
        /// The typed answer.
        answer: Answer,
        /// How.
        via: AnswerVia,
        /// The final transcript (voice only).
        transcript: Option<String>,
    },
    /// Closed without an answer.
    Dismissed {
        /// How.
        gesture: Gesture,
    },
    /// `--expires-in` elapsed.
    Expired,
    /// Withdrawn by the Silicon (`ask cancel`, `unregister`, `logout`).
    Cancelled {
        /// Why (for the UI).
        reason: CancelReason,
    },
}

impl Resolution {
    fn state(&self) -> AskState {
        match self {
            Self::Answered { .. } => AskState::Answered,
            Self::Dismissed { .. } => AskState::Dismissed,
            Self::Expired => AskState::Expired,
            Self::Cancelled { .. } => AskState::Cancelled,
        }
    }
}

/// Builds the outbox row for a ting about `send` and inserts it.
pub(crate) fn queue_ting(
    tx: &Connection,
    send: &SendRow,
    data: &TingData,
    isi: Option<String>,
    subject: &str,
) -> Result<String> {
    queue_ting_for(tx, &send.key, &send.home, data, isi, subject)
}

/// Builds the outbox row for a ting to `key` (through `home`) and inserts it.
pub(crate) fn queue_ting_for(
    tx: &Connection,
    key: &ActorKey,
    home: &HomeRef,
    data: &TingData,
    isi: Option<String>,
    subject: &str,
) -> Result<String> {
    let req = silicon_peek_client::ting::DeliveryRequest::new(
        &key.actor,
        data,
        silicon_peek_client::ting::TingMetadata::new(isi),
    )?;
    let bytes = req.to_bytes()?;
    outbox::insert(
        tx,
        &outbox::NewRow {
            event_id: req.event_id.as_str().to_owned(),
            key: key.clone(),
            home: home.clone(),
            kind: outbox::Kind::Ting,
            request: bytes,
            subject_id: Some(subject.to_owned()),
        },
    )?;
    Ok(req.event_id.as_str().to_owned())
}

/// Where [`Shared::enqueue_send`] put a send.
struct Placed {
    status: SendStatus,
    /// 0 = current, else its place among the waiting (1 = next).
    position: u32,
    /// Waiting plus overflow after the insert.
    waiting: u32,
    /// `--replace`: the send it took over.
    replaced: Option<SendId>,
    receiver: Option<oneshot::Receiver<WaiterMsg>>,
}

impl Shared {
    // ------------------------------------------------------------------ send

    /// `send` (§1.9.3, contract §6.3): validates, stores images, plans
    /// speech, then queues the bubble (or, with `due_at`, schedules it). With
    /// `wait`, the returned receiver gets the ask's final result.
    ///
    /// # Errors
    /// The first failed rule (`side_not_registered`,
    /// `queue_full`, `schedule_full`, validation errors).
    pub async fn handle_send(
        self: &SharedRef,
        caller: &Caller,
        op: SendOp,
        blobs: Vec<Vec<u8>>,
    ) -> Result<(SendResult, Option<oneshot::Receiver<WaiterMsg>>)> {
        op.validate_at(
            &blobs,
            Timestamp::from_unix_ms(self.now_ms()),
            limits::SCHEDULE_SLACK_MS,
        )?;
        if op.due_at.is_some() {
            return self
                .schedule_send(caller, op, blobs)
                .await
                .map(|r| (r, None));
        }
        let key = caller.key.clone();
        let slot = self.send_preconditions(&key).await?;
        let (show, ask) = self
            .store_images(op.show.clone(), op.ask.clone(), blobs)
            .await?;
        let (speech_info, stored_speech, mut warnings_out) = self.plan_speech(caller, &op).await;
        let now = self.now_ms();
        let payload = build_payload(&op, show, ask, stored_speech, now);
        let send_id = SendId::generate();
        let ask_id = payload.ask.as_ref().map(|_| AskId::generate());
        let placed = self
            .enqueue_send(caller, slot, &send_id, ask_id.as_ref(), &payload, &op, now)
            .await?;
        if payload.expires_at.is_some() {
            self.timers_wake.notify_one();
        }
        if placed.position == 0 && !self.ui.is_connected() {
            self.launch_ui_soon();
        }
        let presence = self.ui.presence();
        if !presence.available {
            warnings_out.push(carbon_away_warning(presence.reason, slot));
        } else if presence.paused {
            warnings_out.push(carbon_paused_warning(slot));
        }
        if let Some(w) = self.ting_warning(caller).await {
            warnings_out.push(w);
        }
        if let Some(w) = self.take_fallback_warning(&key).await {
            warnings_out.push(w);
        }
        self.store_send_warnings(&send_id, &warnings_out).await;
        self.record_send(&key, slot, placed.status, &payload, &op, placed.waiting);
        if let Some(old) = &placed.replaced {
            let mut rec = Record::new("send.replaced", "ok").with("slot", slot.get());
            rec.actor = Some((key.org.clone(), key.actor.clone()));
            rec.testing = key.context.is_testing();
            tracing::debug!(send = %send_id, replaced = %old, "a --replace send took over the bubble");
            self.record(rec);
        }
        Ok((
            SendResult {
                send_id,
                ask_id,
                slot,
                status: placed.status,
                speech: speech_info,
                warnings: warnings_out,
                queue_position: Some(placed.position),
                waiting: Some(placed.waiting),
                expires_at: payload.expires_at.map(Timestamp::from_unix_ms),
                schedule_id: None,
                due_at: None,
                tz: op.tz.clone(),
                replaced_send_id: placed.replaced,
            },
            placed.receiver,
        ))
    }

    /// Keeps a send's returned warnings with its history row (first, ahead
    /// of anything recorded since).
    pub(crate) async fn store_send_warnings(&self, send_id: &SendId, warnings_out: &[Warning]) {
        if warnings_out.is_empty() {
            return;
        }
        let sid = send_id.as_str().to_owned();
        let stored: Vec<Value> = warnings_out
            .iter()
            .filter_map(|w| serde_json::to_value(w).ok())
            .collect();
        if let Err(e) = self
            .db
            .call(move |c| prepend_send_warnings(c, &sid, stored))
            .await
        {
            tracing::warn!(error = %e, "recording a send's warnings failed");
        }
    }

    /// `ting_not_enrolled` when answers from this send could not reach the
    /// Silicon: its session records no active Ting enrollment, or Ting
    /// already refused a delivery to it (`recipient_not_registered`, e.g.
    /// after `peek logout --revoke-ting` in another home).
    pub(crate) async fn ting_warning(&self, caller: &Caller) -> Option<Warning> {
        let refused = if caller.ting_subscribed == Some(false) {
            true
        } else {
            let k = caller.key.clone();
            self.db
                .call(move |c| {
                    c.query_row(
                        "SELECT count(*) FROM outbox WHERE kind = 'ting' AND status = 'authority_required'
                           AND last_error_code = 'recipient_not_registered'
                           AND context = ?1 AND org_id = ?2 AND actor_id = ?3",
                        params![k.context_str(), k.org.as_str(), k.actor.as_str()],
                        |r| r.get::<_, i64>(0),
                    )
                    .sql()
                })
                .await
                .is_ok_and(|n| n > 0)
        };
        refused.then(|| Warning {
            code: warnings::TING_NOT_ENROLLED.to_owned(),
            message: format!(
                "{} is not an active Ting recipient for peek, so answers, messages and notifications cannot be delivered; they wait until you enroll",
                caller.key.actor
            ),
            details: Some(json!({"next": "peek ting enroll"})),
        })
    }

    /// The Silicon must hold a position. The UI supplies a built-in visual
    /// when no custom drawing is registered.
    pub(crate) async fn send_preconditions(&self, key: &ActorKey) -> Result<SlotIndex> {
        let k2 = key.clone();
        let (slot, free) = self
            .db
            .call(move |c| {
                let slot = slot_of(c, &k2)?;
                Ok((slot, free_slots(c, k2.context)?))
            })
            .await?;
        let Some(slot) = slot else {
            return Err(side_not_registered(&key.actor, &free));
        };
        Ok(slot)
    }

    /// Stores every image blob in `cache/images/` and rewrites `{"blob":k}`
    /// references to absolute cache paths (the UI hop).
    pub(crate) async fn store_images(
        &self,
        mut show: Option<Show>,
        mut ask: Option<Ask>,
        blobs: Vec<Vec<u8>>,
    ) -> Result<(Option<Show>, Option<Ask>)> {
        let images_dir = self.paths.images_dir();
        let cached: Vec<String> = tokio::task::spawn_blocking(move || -> Result<Vec<String>> {
            blobs.iter().map(|b| cache_image(&images_dir, b)).collect()
        })
        .await
        .map_err(|e| Error::internal(format!("storing images failed: {e}")))??;
        let rewrite = |r: &mut ImageRef| {
            if let ImageRef::Blob { blob } = r
                && let Some(p) = cached.get(*blob)
            {
                *r = ImageRef::Path(p.clone());
            }
        };
        if let Some(s) = show.as_mut() {
            s.images_mut().for_each(rewrite);
            s.validate(ImageHop::Ui)?;
        }
        if let Some(a) = ask.as_mut() {
            a.images_mut().for_each(rewrite);
            a.validate(ImageHop::Ui)?;
        }
        Ok((show, ask))
    }

    /// Picks the voice and whether the audio is cached (§1.9.3, §8.7).
    pub(crate) async fn plan_speech(
        &self,
        caller: &Caller,
        op: &SendOp,
    ) -> (Option<SpeechInfo>, Option<StoredSpeech>, Vec<Warning>) {
        let Some(text) = &op.speak else {
            return (None, None, Vec::new());
        };
        let mut warnings_out = Vec::new();
        // The home's config (`peek config set`): its `voice` is the default
        // voice when the send names none (and does not force another
        // language), its `language` resolves ambiguous detection. The store's
        // config.json wins over the config.sync mirror.
        let stored = caller.store.read_config().ok();
        let fallback = match &stored {
            Some(config) => config.language.clone(),
            None => self.home_language(&caller.home.home_path).await,
        };
        let config_voice = match &stored {
            Some(config) => config.voice.clone(),
            None => {
                self.home_speech_setting(&caller.home.home_path, "voice")
                    .await
            }
        };
        let voice_instructions = match &op.voice_instructions {
            Some(value) => Some(value.clone()),
            None => match &stored {
                Some(config) => config.voice_instructions.clone(),
                None => {
                    self.home_speech_setting(&caller.home.home_path, "voice_instructions")
                        .await
                }
            },
        };
        let explicit = op.voice.clone().or(config_voice);
        let settings = self.settings.get();
        let plan = voice::plan(
            text,
            explicit.as_deref(),
            op.lang.as_deref(),
            fallback.as_deref(),
            &settings.voice_defaults,
        );
        let status = match (&plan.status, &plan.model) {
            (SpeechStatus::Pending, Some(model)) => {
                if self
                    .speech
                    .cache
                    .lookup(&TtsCache::key(
                        model,
                        text,
                        voice_instructions.as_deref(),
                        plan.language.as_deref(),
                    ))
                    .is_some()
                {
                    SpeechStatus::Cached
                } else {
                    SpeechStatus::Pending
                }
            }
            (s, _) => *s,
        };
        if status == SpeechStatus::UnsupportedLanguage {
            warnings_out.push(Warning {
                code: warnings::SPEAK_LANGUAGE_UNSUPPORTED.to_owned(),
                message: format!(
                    "speech is unavailable for {}; the text is shown as a pill instead of spoken",
                    plan.language.as_deref().unwrap_or("this language")
                ),
                details: Some(json!({"language": plan.language})),
            });
        }
        let chars = u32::try_from(text.chars().count()).unwrap_or(u32::MAX);
        (
            Some(SpeechInfo {
                status,
                model: plan.model.clone(),
                chars,
            }),
            Some(StoredSpeech {
                status: status.into(),
                model: plan.model,
                language: plan.language,
                voice_instructions,
            }),
            warnings_out,
        )
    }

    /// Applies the queue rule (contract §6.3) and stores the send, under the
    /// core lock: `--replace` takes over the current bubble (never
    /// `queue_full`); otherwise the send becomes current when there is none,
    /// waits when fewer than five wait, or fails with `queue_full`.
    #[allow(clippy::too_many_arguments)] // one call site; the parts of one send
    async fn enqueue_send(
        self: &SharedRef,
        caller: &Caller,
        slot: SlotIndex,
        send_id: &SendId,
        ask_id: Option<&AskId>,
        payload: &SendPayload,
        op: &SendOp,
        now: i64,
    ) -> Result<Placed> {
        let key = caller.key.clone();
        let mut bubble = Bubble::new(
            send_id.clone(),
            ask_id.cloned(),
            payload.show.is_some(),
            payload.speak.is_some(),
        );
        let notify = serde_json::to_string(&op.notify)
            .map_err(|e| Error::internal(format!("serializing notify failed: {e}")))?;
        let payload_bytes = serde_json::to_vec(payload)
            .map_err(|e| Error::internal(format!("serializing a send failed: {e}")))?;

        let mut core = self.core.lock().await;
        if !op.replace
            && let Some(q) = core.queues.get(&key)
            && q.current.is_some()
            && q.waiting.len() >= limits::QUEUE_MAX
        {
            return Err(queue_full(slot, q, self.held_reason(q.current.as_ref())));
        }
        let mut replaced = None;
        if op.replace
            && let Some(current) = core
                .queues
                .get(&key)
                .and_then(|q| q.current.as_ref().map(|b| b.send_id.clone()))
            && let Some(w) = self
                .withdraw_locked(&mut core, &key, &current, Withdraw::replaced())
                .await?
        {
            if w.pushed {
                bubble.replaces = Some(w.send_id.clone());
            }
            replaced = Some(w.send_id);
        }
        let row = NewSend {
            send_id: send_id.as_str().to_owned(),
            ask_id: ask_id.map(|a| a.as_str().to_owned()),
            key: key.clone(),
            home: caller.home.clone(),
            slot,
            isi: op.isi.clone(),
            payload: payload_bytes,
            notify,
            kind: op.kind(),
            expires_at: payload.expires_at,
            waiter: op_wait(op),
            created_at: now,
            queued_at: now,
            schedule_id: None,
            due_at: None,
            warnings: None,
        };
        self.db.tx(move |tx| insert_send(tx, &row)).await?;
        let receiver = match (ask_id, op.wait) {
            (Some(a), true) => {
                let (tx, rx) = oneshot::channel();
                self.waiters
                    .lock()
                    .map_err(|_| Error::internal("the waiter table is poisoned"))?
                    .insert(a.clone(), tx);
                Some(rx)
            }
            _ => None,
        };
        let queue = core.queues.entry(key.clone()).or_default();
        let position = if queue.current.is_none() {
            queue.current = Some(bubble);
            0
        } else {
            queue.waiting.push_back(bubble);
            u32::try_from(queue.waiting.len()).unwrap_or(u32::MAX)
        };
        debug_assert!(queue.invariants_hold());
        if position == 0 {
            // Shown now, or held (`pushed = false`) until the Carbon or
            // Peek.app is back.
            self.push_current_locked(&mut core, &key).await;
        } else {
            self.emit_queue_state_locked(&mut core, &key).await;
        }
        let q = core.queues.get(&key);
        let waiting = q.map_or(0, crate::state::ActorQueue::waiting_count);
        let status = if position == 0
            && self
                .held_reason(q.and_then(|q| q.current.as_ref()))
                .is_none()
        {
            SendStatus::Showing
        } else {
            SendStatus::Queued
        };
        Ok(Placed {
            status,
            position,
            waiting,
            replaced,
            receiver,
        })
    }

    fn record_send(
        &self,
        key: &ActorKey,
        slot: SlotIndex,
        status: SendStatus,
        payload: &SendPayload,
        op: &SendOp,
        waiting: u32,
    ) {
        let mut rec = Record::new(
            match status {
                SendStatus::Showing => "send.displayed",
                SendStatus::Queued => "send.queued",
                SendStatus::Scheduled => "send.scheduled",
            },
            "ok",
        )
        .with("slot", slot.get());
        if status == SendStatus::Scheduled {
            rec = rec.with("scheduled", true);
        } else {
            rec = rec.with("queue_waiting", waiting);
        }
        if let Some(s) = &payload.show {
            rec = rec
                .with("show_elements", s.elements.len())
                .with("show_kinds", json!(s.kinds()));
        }
        if let Some(a) = &payload.ask {
            rec = rec
                .with("ask_type", a.ask_type().as_str())
                .with("options_count", a.options().len());
        }
        if let Some(t) = &op.speak {
            rec = rec.with("speak_chars", t.chars().count());
        }
        rec.actor = Some((key.org.clone(), key.actor.clone()));
        rec.isi.clone_from(&op.isi);
        rec.testing = key.context.is_testing();
        self.record(rec);
    }

    /// The `voice` of a home's mirrored config (`config.sync`).
    async fn home_speech_setting(&self, home_path: &str, field: &'static str) -> Option<String> {
        let hp = home_path.to_owned();
        self.db
            .call(move |c| {
                let cfg: Option<Option<String>> = c
                    .query_row("SELECT config FROM homes WHERE home_path = ?1", [hp], |r| {
                        r.get(0)
                    })
                    .optional()
                    .sql()?;
                Ok(cfg
                    .flatten()
                    .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                    .and_then(|v| v.get(field).and_then(Value::as_str).map(str::to_owned))
                    .filter(|v| {
                        if field == "voice" {
                            check_voice(v).is_ok()
                        } else {
                            check_voice_instructions(v).is_ok()
                        }
                    }))
            })
            .await
            .ok()
            .flatten()
    }

    async fn home_language(&self, home_path: &str) -> Option<String> {
        let hp = home_path.to_owned();
        self.db
            .call(move |c| {
                let cfg: Option<Option<String>> = c
                    .query_row("SELECT config FROM homes WHERE home_path = ?1", [hp], |r| {
                        r.get(0)
                    })
                    .optional()
                    .sql()?;
                Ok(cfg
                    .flatten()
                    .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                    .and_then(|v| v.get("language").and_then(Value::as_str).map(str::to_owned)))
            })
            .await
            .ok()
            .flatten()
    }

    // ------------------------------------------------------------- pushing

    /// Whether the connected Peek.app reports `shown` (build 1002 and up).
    /// Older apps get 0.1.1's behaviour: shown (and spoken) at push time.
    pub(crate) fn ui_reports_shown(&self) -> bool {
        self.ui.current().is_some_and(|l| l.build >= SHOWN_OP_BUILD)
    }

    /// Why a Silicon's current bubble is not on screen, if it is not:
    /// Peek.app not connected (or an update swap), the Carbon away, or
    /// Peek paused.
    pub(crate) fn held_reason(&self, current: Option<&Bubble>) -> Option<HeldReason> {
        let b = current?;
        if !self.ui.is_connected()
            || self
                .update_swapping
                .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Some(HeldReason::AppNotRunning);
        }
        let presence = self.ui.presence();
        if !presence.available {
            Some(HeldReason::CarbonAway)
        } else if presence.paused {
            Some(HeldReason::Paused)
        } else if b.pushed {
            None
        } else {
            Some(HeldReason::AppNotRunning)
        }
    }

    /// Shows the Silicon's current bubble on the UI if it is not there yet.
    /// Called with the core lock held. peekd pushes while the Carbon paused
    /// Peek (the ⌃⌘N summon must find a pending ask in the app); it does not
    /// push while the Carbon is away or Peek.app is swapping builds.
    pub(crate) async fn push_current_locked(self: &SharedRef, core: &mut Core, key: &ActorKey) {
        if !self.ui.is_connected()
            || !self.ui.carbon_available()
            || self
                .update_swapping
                .load(std::sync::atomic::Ordering::SeqCst)
        {
            // Stays `pushed = false`: shown by the next UI connection, by
            // `push_all` when the Carbon is back (`presence`), or when a
            // pending update backs off. No speech starts before that.
            return;
        }
        let Some(queue) = core.queues.get(key) else {
            return;
        };
        let queued_behind = queue.waiting_count();
        let Some(bubble) = queue.current.clone() else {
            return;
        };
        if bubble.pushed {
            return;
        }
        let sid = bubble.send_id.as_str().to_owned();
        let k = key.clone();
        let loaded = self
            .db
            .call(move |c| {
                let send = load_send(c, &sid)?;
                let slot = slot_of(c, &k)?;
                Ok((send, slot))
            })
            .await;
        let (send, slot) = match loaded {
            Ok((Some(s), slot)) => {
                let slot = slot.unwrap_or(s.slot);
                (s, slot)
            }
            Ok((None, _)) => {
                tracing::error!(send = %bubble.send_id, "a queued send has no database row; dropping it");
                if let Some(q) = core.queues.get_mut(key) {
                    q.current = None;
                }
                Box::pin(self.advance_locked(core, key)).await;
                return;
            }
            Err(e) => {
                tracing::error!(error = %e, "loading a send to show failed");
                return;
            }
        };
        if send.expires_at.is_some_and(|e| e <= self.now_ms()) {
            // Past its deadline while it waited: never shown; the timer
            // expires it (and advances the queue) right away.
            self.timers_wake.notify_one();
            return;
        }
        let event = peek_show_event(&send, &bubble, slot, queued_behind, key);
        if !self.ui.event(&event, Vec::new()) {
            return;
        }
        let legacy_ui = !self.ui_reports_shown();
        let deadline = if legacy_ui {
            self.bubble_deadline(&send, &bubble)
        } else {
            None
        };
        if let Some(q) = core.queues.get_mut(key) {
            q.last_badge = Some(queued_behind);
            if let Some(b) = q.current.as_mut() {
                b.pushed = true;
                b.replaces = None;
                if legacy_ui {
                    b.shown = true;
                    b.deadline = deadline;
                }
            }
        }
        if deadline.is_some() {
            self.timers_wake.notify_one();
        }
        if legacy_ui {
            if send.shown_at.is_none() {
                self.first_show(&send, key, slot).await;
            }
        } else if send.payload.ask.is_some() {
            // Pre-warm the session an answer will need while the bubble
            // slides in; speech waits for the app's `shown`.
            self.prewarm(send.home.clone(), key.clone());
        }
    }

    /// When peekd closes a non-ask bubble itself if the UI never reports it
    /// done: its duration (or speech time) plus a grace period.
    fn bubble_deadline(&self, send: &SendRow, bubble: &Bubble) -> Option<Instant> {
        if bubble.ask_id.is_some() {
            return None;
        }
        let chars = send.payload.speak.as_ref().map_or(0, |t| t.chars().count());
        let speech_ms = u64::try_from(chars).unwrap_or(0).saturating_mul(1000) / 14 + 1500;
        let visible = send
            .payload
            .duration_ms
            .unwrap_or(0)
            .max(if bubble.has_speak { speech_ms } else { 0 });
        Some(
            Instant::now()
                + Duration::from_millis(visible.saturating_add(speech_ms))
                + self.cfg.timings.bubble_grace,
        )
    }

    /// The first time a send appears (the UI's `shown`, or `peek.show`
    /// reaching an app older than build 1002): record `shown_at`, start its
    /// speech, pre-warm the session an answer will need, and send
    /// `peek.send.shown` when the Silicon asked for it (`--notify shown`) or
    /// the send was scheduled.
    async fn first_show(self: &SharedRef, send: &SendRow, key: &ActorKey, slot: SlotIndex) {
        let sid = send.send_id.as_str().to_owned();
        let now = self.now_ms();
        if let Err(e) = self
            .db
            .call(move |c| {
                c.execute(
                    "UPDATE sends SET shown_at = ?2 WHERE send_id = ?1 AND shown_at IS NULL",
                    params![sid, now],
                )
                .sql()
            })
            .await
        {
            tracing::warn!(error = %e, "recording shown_at failed");
        }
        if let (Some(text), Some(speech)) = (&send.payload.speak, &send.payload.speech)
            && speech.status.to_speak()
            && let Some(model) = &speech.model
        {
            self.start_tts(TtsJob {
                send_id: send.send_id.clone(),
                key: key.clone(),
                home: send.home.clone(),
                model: if model.starts_with("aura-") {
                    "Kore".to_owned()
                } else {
                    model.clone()
                },
                text: text.clone(),
                voice_instructions: speech.voice_instructions.clone(),
                language: speech.language.clone(),
            });
        }
        if send.payload.ask.is_some() {
            self.prewarm(send.home.clone(), key.clone());
        }
        if send.notify.contains(&Notify::Shown) || send.schedule_id.is_some() {
            let ask_id = self
                .db
                .call({
                    let sid = send.send_id.as_str().to_owned();
                    move |c| ask_of_send(c, &sid)
                })
                .await
                .ok()
                .flatten()
                .map(|a| a.ask_id);
            let data = TingData::SendShown(SendShown {
                schema: SchemaV1,
                send_id: send.send_id.clone(),
                ask_id,
                kind: send.kind.clone(),
                created_at: Timestamp::from_unix_ms(send.created_at),
                shown_at: Timestamp::from_unix_ms(now.max(send.created_at)),
                scheduled: send.schedule_id.is_some(),
                schedule_id: send.schedule_id.clone(),
                slot,
                context: key.context.data_context(),
            });
            if let Err(e) = self.queue_send_ting(send, data).await {
                tracing::warn!(send = %send.send_id, error = %e, "queueing peek.send.shown failed");
            }
        }
        let mut rec = Record::new("send.shown", "ok").with("slot", slot.get());
        if send.schedule_id.is_some() {
            rec = rec.with("scheduled", true);
        }
        rec.actor = Some((key.org.clone(), key.actor.clone()));
        rec.isi.clone_from(&send.isi);
        rec.testing = key.context.is_testing();
        self.record(rec);
    }

    /// `shown` (UI op, contract §6.8): Peek.app began presenting the current
    /// bubble. The first report records `shown_at`, starts the speech, arms
    /// the watchdog of a non-ask bubble and sends `peek.send.shown` when
    /// asked for. Reports for other sends, or repeats, change nothing.
    ///
    /// # Errors
    /// Database failures.
    pub async fn ui_shown(self: &SharedRef, op: Shown) -> Result<()> {
        let mut core = self.core.lock().await;
        let Some(key) = core.queue_of(&op.send_id) else {
            return Ok(());
        };
        let Some(bubble) = core
            .queues
            .get(&key)
            .and_then(|q| q.current.clone())
            .filter(|b| b.send_id == op.send_id && b.pushed && !b.shown)
        else {
            return Ok(());
        };
        let sid = op.send_id.as_str().to_owned();
        let k = key.clone();
        let (send, slot) = self
            .db
            .call(move |c| Ok((load_send(c, &sid)?, slot_of(c, &k)?)))
            .await?;
        let Some(send) = send else {
            return Ok(());
        };
        let slot = slot.unwrap_or(send.slot);
        let deadline = if send.shown_at.is_none() || bubble.ask_id.is_none() {
            self.bubble_deadline(&send, &bubble)
        } else {
            None
        };
        if let Some(b) = core.queues.get_mut(&key).and_then(|q| q.current.as_mut()) {
            b.shown = true;
            if deadline.is_some() {
                b.deadline = deadline;
            }
        }
        if deadline.is_some() {
            self.timers_wake.notify_one();
        }
        if send.shown_at.is_none() {
            self.first_show(&send, &key, slot).await;
        }
        Ok(())
    }

    /// Promotes the next waiting send after the current one closed (then
    /// the first overflow send takes the freed waiting spot).
    pub(crate) async fn advance_locked(self: &SharedRef, core: &mut Core, key: &ActorKey) {
        if let Some(q) = core.queues.get_mut(key)
            && q.current.is_none()
        {
            q.current = q.waiting.pop_front();
            q.last_badge = None;
            if q.current.is_none() {
                // Only overflow left (a restart with a broken invariant).
                q.current = q.overflow.pop_front();
            }
        }
        self.promote_overflow_locked(core, key).await;
        if core
            .queues
            .get(key)
            .is_some_and(crate::state::ActorQueue::is_empty)
        {
            core.queues.remove(key);
            return;
        }
        debug_assert!(
            core.queues
                .get(key)
                .is_none_or(crate::state::ActorQueue::invariants_hold)
        );
        self.push_current_locked(core, key).await;
        self.emit_queue_state_locked(core, key).await;
    }

    /// Re-shows every current bubble on a fresh UI connection.
    pub async fn push_all(self: &SharedRef) {
        let mut core = self.core.lock().await;
        let keys: Vec<ActorKey> = core.queues.keys().cloned().collect();
        for k in keys {
            self.push_current_locked(&mut core, &k).await;
        }
    }

    /// The UI went away: non-ask bubbles it had shown are closed
    /// (`ui_disconnected`); asks, and bubbles it never reported shown, stay
    /// and are shown by the next UI.
    pub async fn on_ui_disconnect(self: &SharedRef) {
        let mut core = self.core.lock().await;
        let mut closed = Vec::new();
        for (k, q) in &mut core.queues {
            q.last_badge = None;
            let Some(b) = q.current.as_mut() else {
                continue;
            };
            if b.ask_id.is_none() && b.pushed && b.shown {
                closed.push((k.clone(), b.send_id.clone()));
                q.current = None;
            } else {
                b.pushed = false;
                b.shown = false;
                b.deadline = None;
            }
        }
        for (_, s) in &closed {
            self.speech.cancel(s);
        }
        let ids: Vec<String> = closed.iter().map(|(_, s)| s.as_str().to_owned()).collect();
        let now = self.now_ms();
        if let Err(e) = self
            .db
            .tx(move |tx| {
                for id in ids {
                    tx.execute(
                        "UPDATE sends SET closed_at = ?2, close_reason = 'ui_disconnected' WHERE send_id = ?1 AND closed_at IS NULL",
                        params![id, now],
                    )
                    .sql()?;
                }
                Ok(())
            })
            .await
        {
            tracing::warn!(error = %e, "closing bubbles after a UI disconnect failed");
        }
        for (k, _) in closed {
            self.advance_locked(&mut core, &k).await;
        }
        core.queues.retain(|_, q| !q.is_empty());
    }

    /// Rebuilds the queues from the database at start: open sends per
    /// Silicon in queue order (`overflow`, `queued_at`, `send_id`): the first
    /// becomes current, up to five wait, due scheduled sends marked overflow
    /// wait for room (and take any free spot now). Non-ask bubbles that were
    /// already on screen before the restart are closed (`daemon_restarted`).
    ///
    /// # Errors
    /// Database failures.
    pub async fn load_queues(&self) -> Result<usize> {
        let now = self.now_ms();
        let rows = self
            .db
            .tx(move |tx| {
                tx.execute(
                    "UPDATE sends SET closed_at = ?1, close_reason = 'daemon_restarted'
                     WHERE closed_at IS NULL AND shown_at IS NOT NULL
                       AND send_id NOT IN (SELECT send_id FROM asks WHERE state = 'pending')",
                    [now],
                )
                .sql()?;
                let mut st = tx
                    .prepare(&format!(
                        "SELECT {SEND_COLS} FROM sends WHERE closed_at IS NULL
                         ORDER BY overflow, COALESCE(queued_at, created_at), send_id"
                    ))
                    .sql()?;
                let sends = st
                    .query_map([], send_from_row)
                    .sql()?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .sql()?;
                let mut out = Vec::new();
                for s in sends {
                    let s = s?;
                    let ask = ask_of_send(tx, s.send_id.as_str())?;
                    out.push((s, ask));
                }
                Ok(out)
            })
            .await?;
        let mut core = self.core.lock().await;
        let mut n = 0;
        let mut promoted: Vec<String> = Vec::new();
        let mut demoted: Vec<String> = Vec::new();
        for (send, ask) in rows {
            let ask_id = match ask {
                Some(a) if a.state == AskState::Pending => Some(a.ask_id),
                Some(_) => continue,
                None => None,
            };
            let q = core.queues.entry(send.key.clone()).or_default();
            let bubble = Bubble::new(
                send.send_id,
                ask_id,
                send.payload.show.is_some(),
                send.payload.speak.is_some(),
            );
            let id = bubble.send_id.as_str().to_owned();
            if q.current.is_none() {
                q.current = Some(bubble);
                if send.overflow {
                    promoted.push(id);
                }
            } else if q.waiting.len() < limits::QUEUE_MAX && q.overflow.is_empty() {
                q.waiting.push_back(bubble);
                if send.overflow {
                    promoted.push(id);
                }
            } else {
                q.overflow.push_back(bubble);
                if !send.overflow {
                    demoted.push(id);
                }
            }
            debug_assert!(q.invariants_hold());
            n += 1;
        }
        drop(core);
        if !promoted.is_empty() || !demoted.is_empty() {
            self.db
                .tx(move |tx| {
                    for id in promoted {
                        tx.execute("UPDATE sends SET overflow = 0 WHERE send_id = ?1", [id])
                            .sql()?;
                    }
                    for id in demoted {
                        tx.execute("UPDATE sends SET overflow = 1 WHERE send_id = ?1", [id])
                            .sql()?;
                    }
                    Ok(())
                })
                .await?;
        }
        Ok(n)
    }

    /// Whether anything waits for the Carbon (a pending ask or a queued
    /// send). Scheduled sends that are not due yet do not count.
    pub async fn has_pending_work(&self) -> bool {
        !self.core.lock().await.queues.is_empty()
    }

    // ---------------------------------------------------------- closing

    /// Closes a non-ask send (`shown.done`, `dismissed`, timeouts) and shows
    /// the next one. Returns the row as it was.
    ///
    /// # Errors
    /// Database failures.
    pub async fn close_send(
        self: &SharedRef,
        send_id: &SendId,
        reason: &str,
    ) -> Result<Option<SendRow>> {
        let mut core = self.core.lock().await;
        let sid = send_id.as_str().to_owned();
        let reason_s = reason.to_owned();
        let now = self.now_ms();
        let row = self
            .db
            .tx(move |tx| {
                let row = load_send(tx, &sid)?;
                tx.execute(
                    "UPDATE sends SET closed_at = ?2, close_reason = ?3 WHERE send_id = ?1 AND closed_at IS NULL",
                    params![sid, now, reason_s],
                )
                .sql()?;
                Ok(row)
            })
            .await?;
        if let Some(key) = core.queue_of(send_id)
            && let Some(from) = take_from_queue(&mut core, &key, send_id)
        {
            self.after_removal_locked(&mut core, &key, from).await;
        }
        Ok(row)
    }

    /// Hands a final result to a live `--wait` connection; true when the CLI
    /// took it (then no ting is sent).
    async fn hand_to_waiter(&self, ask_id: &AskId, result: AskResult) -> bool {
        let waiter = self.waiters.lock().ok().and_then(|mut w| w.remove(ask_id));
        let Some(tx) = waiter else {
            return false;
        };
        let (ack_tx, ack_rx) = oneshot::channel();
        if tx
            .send(WaiterMsg {
                result,
                ack: ack_tx,
            })
            .is_err()
        {
            return false;
        }
        // The connection waits `waiter_ack` for the CLI's ack itself; the
        // extra second makes sure it, not this timer, decides.
        matches!(
            tokio::time::timeout(self.cfg.timings.waiter_ack + Duration::from_secs(1), ack_rx)
                .await,
            Ok(Ok(true))
        )
    }

    /// Ends an ask (§1.9.4–§1.9.6): records the outcome, delivers it through
    /// the waiter or the outbox, withdraws the bubble when peekd initiated
    /// the end, and shows the next queued send.
    ///
    /// # Errors
    /// `ask_not_found` when the ask is unknown or no longer pending.
    pub async fn resolve_ask(
        self: &SharedRef,
        ask_id: &AskId,
        res: Resolution,
    ) -> Result<AskState> {
        let (ask, send) = self.claim_ask(ask_id).await?;
        let now = self.now_ms();
        let state = res.state();
        let (answer, via, transcript) = match &res {
            Resolution::Answered {
                answer,
                via,
                transcript,
            } => (Some(answer.clone()), Some(*via), transcript.clone()),
            _ => (None, None, None),
        };
        let result = AskResult {
            ask_id: ask_id.clone(),
            state,
            answer,
            via,
            transcript,
            answered_at: matches!(res, Resolution::Answered { .. })
                .then(|| Timestamp::from_unix_ms(now)),
        };
        let waited = self.hand_to_waiter(ask_id, result).await;

        let mut core = self.core.lock().await;
        core.resolving.remove(ask_id);
        let committed = self
            .commit_resolution(&ask, &send, &res, now, waited)
            .await?;
        if committed {
            self.outbox_wake.notify_one();
        }
        if let Some(w) = self.waiters.lock().ok().as_mut() {
            w.remove(ask_id);
        }
        let key = send.key.clone();
        let from = take_from_queue(&mut core, &key, &send.send_id);
        let was_pushed = matches!(from, Some(Slotted::Current { pushed: true }));
        if was_pushed && let Resolution::Expired | Resolution::Cancelled { .. } = &res {
            let reason = match &res {
                Resolution::Cancelled { reason } => *reason,
                _ => CancelReason::Expired,
            };
            self.speech.cancel(&send.send_id);
            self.ui.event(
                &PeekCancel {
                    send_id: send.send_id.clone(),
                    reason,
                },
                Vec::new(),
            );
        }
        if let Some(from) = from {
            self.after_removal_locked(&mut core, &key, from).await;
        }
        drop(core);
        if !committed {
            // No ting references the recording; with one, the outbox deletes
            // it once the delivery ends (§1.7).
            let _ = std::fs::remove_file(self.paths.recording(ask_id.as_str()));
        }
        let mut rec = Record::new("ask.resolved", "ok")
            .with(
                "ask_type",
                send.payload
                    .ask
                    .as_ref()
                    .map_or("", |a| a.ask_type().as_str()),
            )
            .with("status", enum_str(&state))
            .with(
                "answer_latency_ms",
                now.saturating_sub(send.shown_at.unwrap_or(send.created_at)),
            );
        if let Some(v) = via {
            rec = rec.with("input", enum_str(&v));
        }
        rec.actor = Some((key.org.clone(), key.actor.clone()));
        rec.testing = key.context.is_testing();
        self.record(rec);
        Ok(state)
    }

    /// Phase 1 of a resolution: the ask must be pending and not already
    /// being resolved by another path (an answer racing an expiry).
    async fn claim_ask(&self, ask_id: &AskId) -> Result<(AskRow, SendRow)> {
        let aid = ask_id.as_str().to_owned();
        let mut core = self.core.lock().await;
        let loaded = self
            .db
            .call(move |c| {
                let ask = load_ask(c, &aid)?;
                let send = match &ask {
                    Some(a) => load_send(c, a.send_id.as_str())?,
                    None => None,
                };
                Ok((ask, send))
            })
            .await?;
        let (Some(ask), Some(send)) = loaded else {
            return Err(Error::new(
                ErrorCode::AskNotFound,
                format!("no ask {ask_id} exists"),
            ));
        };
        if ask.state != AskState::Pending || !core.resolving.insert(ask_id.clone()) {
            return Err(Error::new(
                ErrorCode::AskNotFound,
                format!(
                    "ask {ask_id} is no longer pending (it is {})",
                    enum_str(&ask.state)
                ),
            )
            .with_details(json!({"state": ask.state})));
        }
        Ok((ask, send))
    }

    /// Phase 3: one transaction records the outcome, closes the send, and
    /// queues the ting unless a `--wait` connection took the result. Nothing
    /// is written when the ask closed meanwhile (the Silicon's `--replace`).
    /// Returns whether an outbox row was added.
    async fn commit_resolution(
        &self,
        ask: &AskRow,
        send: &SendRow,
        res: &Resolution,
        now: i64,
        waited: bool,
    ) -> Result<bool> {
        let ting = if waited {
            None
        } else {
            self.ting_for_ask(ask, send, res, now).await
        };
        let (answer, via, transcript) = match res {
            Resolution::Answered {
                answer,
                via,
                transcript,
            } => (Some(answer.clone()), Some(*via), transcript.clone()),
            _ => (None, None, None),
        };
        let aid = ask.ask_id.as_str().to_owned();
        let sid = send.send_id.as_str().to_owned();
        let send_c = send.clone();
        let answer_bytes = answer
            .as_ref()
            .map(serde_json::to_vec)
            .transpose()
            .map_err(|e| Error::internal(format!("serializing an answer failed: {e}")))?;
        let state_s = enum_str(&res.state());
        let via_s = via.map(|v| enum_str(&v));
        let isi = send.isi.clone();
        self.db
            .tx(move |tx| {
                let still_pending: Option<String> = tx
                    .query_row("SELECT state FROM asks WHERE ask_id = ?1", [&aid], |r| r.get(0))
                    .optional()
                    .sql()?;
                if still_pending.as_deref() != Some("pending") {
                    return Ok(false);
                }
                let (delivered_via, event_id) = if waited {
                    (Some("wait".to_owned()), None)
                } else if let Some(data) = ting {
                    let ev = queue_ting(tx, &send_c, &data, isi, &aid)?;
                    (Some("ting".to_owned()), Some(ev))
                } else {
                    (None, None)
                };
                tx.execute(
                    "UPDATE asks SET state = ?2, answer = ?3, via = ?4, transcript = ?5, answered_at = ?6,
                            closed_at = ?7, delivered_via = ?8, event_id = ?9, waiter = 0
                     WHERE ask_id = ?1 AND state = 'pending'",
                    params![
                        aid,
                        state_s,
                        answer_bytes,
                        via_s,
                        transcript,
                        answer_bytes.as_ref().map(|_| now),
                        now,
                        delivered_via,
                        event_id
                    ],
                )
                .sql()?;
                tx.execute(
                    "UPDATE sends SET closed_at = ?2, close_reason = ?3 WHERE send_id = ?1 AND closed_at IS NULL",
                    params![sid, now, state_s],
                )
                .sql()?;
                Ok(event_id.is_some())
            })
            .await
    }

    async fn ting_for_ask(
        &self,
        ask: &AskRow,
        send: &SendRow,
        res: &Resolution,
        now: i64,
    ) -> Option<TingData> {
        let q = send.payload.ask.as_ref()?;
        let k = send.key.clone();
        let slot = self
            .db
            .call(move |c| slot_of(c, &k))
            .await
            .ok()
            .flatten()
            .unwrap_or(send.slot);
        let asked_at_ms = send.shown_at.unwrap_or(ask.created_at);
        let asked_at = Timestamp::from_unix_ms(asked_at_ms);
        let at = Timestamp::from_unix_ms(now.max(asked_at_ms));
        let context = send.key.context.data_context();
        Some(match res {
            Resolution::Answered {
                answer,
                via,
                transcript,
            } => TingData::AskAnswered(AskAnswered {
                schema: SchemaV1,
                ask_id: ask.ask_id.clone(),
                send_id: send.send_id.clone(),
                question: q.question.clone(),
                ask_type: q.ask_type(),
                answer: answer.clone(),
                via: *via,
                transcript: if *via == AnswerVia::Voice {
                    transcript.clone()
                } else {
                    None
                },
                asked_at,
                answered_at: at,
                slot,
                context,
            }),
            Resolution::Dismissed { gesture } => TingData::AskDismissed(AskDismissed {
                schema: SchemaV1,
                ask_id: ask.ask_id.clone(),
                send_id: send.send_id.clone(),
                question: q.question.clone(),
                ask_type: q.ask_type(),
                gesture: *gesture,
                asked_at,
                dismissed_at: at,
                slot,
                context,
            }),
            Resolution::Expired => TingData::AskExpired(AskExpired {
                schema: SchemaV1,
                ask_id: ask.ask_id.clone(),
                send_id: send.send_id.clone(),
                question: q.question.clone(),
                ask_type: q.ask_type(),
                asked_at,
                expired_at: at,
                slot,
                context,
                shown: Some(send.shown_at.is_some()),
            }),
            // Nothing is sent for a Silicon's own actions (§3.1).
            Resolution::Cancelled { .. } => return None,
        })
    }

    /// Cancels everything of a Silicon (unregister, logout): its pending
    /// asks, the sends on screen, waiting and waiting for room, and its
    /// scheduled sends. No tings are sent.
    ///
    /// # Errors
    /// Database failures.
    pub async fn cancel_actor_bubbles(
        self: &SharedRef,
        key: &ActorKey,
        reason: CancelReason,
    ) -> Result<CancelledAll> {
        let mut core = self.core.lock().await;
        let mut out = CancelledAll::default();
        let ids: Vec<SendId> = core
            .queues
            .get(key)
            .map(|q| {
                q.current
                    .iter()
                    .chain(q.waiting.iter())
                    .chain(q.overflow.iter())
                    .map(|b| b.send_id.clone())
                    .collect()
            })
            .unwrap_or_default();
        let how = Withdraw {
            ui_reason: reason,
            close_reason: "unregistered",
            ask_state: AskState::Cancelled,
            ting: None,
        };
        for id in ids {
            if let Some(w) = self
                .withdraw_locked(&mut core, key, &id, how.clone())
                .await?
            {
                match w.ask_id {
                    Some(a) => out.asks.push(a),
                    None => out.sends.push(w.send_id),
                }
            }
        }
        // Anything the queue did not hold (an ask being answered right now
        // is left to that answer) and every scheduled send.
        let resolving: Vec<String> = core
            .resolving
            .iter()
            .map(|a| a.as_str().to_owned())
            .collect();
        let (orphans, scheduled) = self.cancel_actor_rows(key, resolving).await?;
        for a in orphans {
            if let Ok(id) = AskId::parse(&a) {
                if let Some(tx) = self.waiters.lock().ok().and_then(|mut w| w.remove(&id)) {
                    let (ack, _) = oneshot::channel();
                    let _ = tx.send(WaiterMsg {
                        result: cancelled_result(&id, AskState::Cancelled),
                        ack,
                    });
                }
                if !out.asks.contains(&id) {
                    out.asks.push(id);
                }
            }
        }
        out.scheduled = scheduled
            .iter()
            .filter_map(|s| ScheduleId::parse(s).ok())
            .collect();
        if core
            .queues
            .get(key)
            .is_some_and(crate::state::ActorQueue::is_empty)
        {
            core.queues.remove(key);
        }
        Ok(out)
    }

    /// The database side of [`Shared::cancel_actor_bubbles`]: pending asks
    /// and open sends of `key` the queue did not hold (except asks in
    /// `resolving`), and every scheduled send. Returns the orphan asks and
    /// the scheduled IDs.
    async fn cancel_actor_rows(
        &self,
        key: &ActorKey,
        resolving: Vec<String>,
    ) -> Result<(Vec<String>, Vec<String>)> {
        let k = key.clone();
        let now = self.now_ms();
        self.db
            .tx(move |tx| {
                let mut st = tx
                    .prepare(
                        "SELECT a.ask_id FROM asks a JOIN sends s ON s.send_id = a.send_id
                         WHERE a.state = 'pending' AND s.context = ?1 AND s.org_id = ?2 AND s.actor_id = ?3
                         ORDER BY a.created_at",
                    )
                    .sql()?;
                let asks: Vec<String> = st
                    .query_map(params![k.context_str(), k.org.as_str(), k.actor.as_str()], |r| r.get(0))
                    .sql()?
                    .collect::<rusqlite::Result<_>>()
                    .sql()?;
                let mut orphans = Vec::new();
                for a in asks.into_iter().filter(|a| !resolving.contains(a)) {
                    tx.execute(
                        "UPDATE asks SET state = 'cancelled', closed_at = ?2, waiter = 0 WHERE ask_id = ?1 AND state = 'pending'",
                        params![a, now],
                    )
                    .sql()?;
                    tx.execute(
                        "UPDATE sends SET closed_at = ?2, close_reason = 'cancelled'
                         WHERE send_id = (SELECT send_id FROM asks WHERE ask_id = ?1) AND closed_at IS NULL",
                        params![a, now],
                    )
                    .sql()?;
                    orphans.push(a);
                }
                tx.execute(
                    "UPDATE sends SET closed_at = ?4, close_reason = 'unregistered'
                     WHERE closed_at IS NULL AND context = ?1 AND org_id = ?2 AND actor_id = ?3
                       AND send_id NOT IN (SELECT send_id FROM asks WHERE state = 'pending')",
                    params![k.context_str(), k.org.as_str(), k.actor.as_str(), now],
                )
                .sql()?;
                let mut st = tx
                    .prepare(
                        "SELECT schedule_id FROM scheduled WHERE context = ?1 AND org_id = ?2 AND actor_id = ?3
                         ORDER BY due_at, schedule_id",
                    )
                    .sql()?;
                let scheduled: Vec<String> = st
                    .query_map(params![k.context_str(), k.org.as_str(), k.actor.as_str()], |r| r.get(0))
                    .sql()?
                    .collect::<rusqlite::Result<_>>()
                    .sql()?;
                tx.execute(
                    "DELETE FROM scheduled WHERE context = ?1 AND org_id = ?2 AND actor_id = ?3",
                    params![k.context_str(), k.org.as_str(), k.actor.as_str()],
                )
                .sql()?;
                Ok((orphans, scheduled))
            })
            .await
    }

    // --------------------------------------------------------- UI ops

    /// `answer` (click or keyboard).
    ///
    /// # Errors
    /// `ask_not_found`, `invalid_input` for a value that does not answer it.
    pub async fn ui_answer(self: &SharedRef, op: AnswerOp) -> Result<()> {
        let aid = op.ask_id.as_str().to_owned();
        let (ask, send) = self
            .db
            .call(move |c| {
                let a = load_ask(c, &aid)?;
                let s = match &a {
                    Some(a) => load_send(c, a.send_id.as_str())?,
                    None => None,
                };
                Ok((a, s))
            })
            .await?;
        let (Some(ask), Some(send)) = (ask, send) else {
            return Err(Error::new(
                ErrorCode::AskNotFound,
                format!("no ask {} exists", op.ask_id),
            ));
        };
        if ask.send_id != op.send_id {
            return Err(Error::invalid_input(format!(
                "ask {} belongs to send {}, not {}",
                op.ask_id, ask.send_id, op.send_id
            )));
        }
        let q = send.payload.ask.as_ref().ok_or_else(|| {
            Error::internal(format!(
                "send {} has an ask row but no question",
                send.send_id
            ))
        })?;
        let answer = q.resolve_answer(&op.value)?;
        let via = match op.via {
            silicon_peek_client::ipc::ui::UiAnswerVia::Click => AnswerVia::Click,
            silicon_peek_client::ipc::ui::UiAnswerVia::Keyboard => AnswerVia::Keyboard,
        };
        self.resolve_ask(
            &op.ask_id,
            Resolution::Answered {
                answer,
                via,
                transcript: None,
            },
        )
        .await
        .map(|_| ())
    }

    /// `dismissed`.
    ///
    /// # Errors
    /// Database failures.
    pub async fn ui_dismissed(self: &SharedRef, op: Dismissed) -> Result<()> {
        let sid = op.send_id.as_str().to_owned();
        let (send, ask) = self
            .db
            .call(move |c| Ok((load_send(c, &sid)?, ask_of_send(c, &sid)?)))
            .await?;
        let Some(send) = send else {
            return Err(Error::invalid_input(format!(
                "no send {} exists",
                op.send_id
            )));
        };
        // A double gesture stops the audio; so does Esc on an ask (two Escs
        // mean "make it all go away", contract §0.2 decision 7).
        if matches!(op.gesture, Gesture::DownArrowDouble | Gesture::EscDouble)
            || (op.gesture == Gesture::Esc && send.payload.ask.is_some())
        {
            self.speech.cancel(&op.send_id);
        }
        if let Some(a) = ask.filter(|a| a.state == AskState::Pending) {
            self.resolve_ask(
                &a.ask_id,
                Resolution::Dismissed {
                    gesture: op.gesture,
                },
            )
            .await?;
            return Ok(());
        }
        if send.closed_at.is_some() {
            return Ok(());
        }
        self.close_send(&op.send_id, "dismissed").await?;
        if send.payload.show.is_some() && send.notify.contains(&Notify::ShowDismissed) {
            let now = self.now_ms();
            let data = TingData::ShowDismissed(ShowDismissed {
                schema: SchemaV1,
                send_id: send.send_id.clone(),
                gesture: op.gesture,
                visible_ms: u64::try_from(
                    now.saturating_sub(send.shown_at.unwrap_or(send.created_at)),
                )
                .unwrap_or(0),
                dismissed_at: Timestamp::from_unix_ms(now),
                slot: send.slot,
                context: send.key.context.data_context(),
            });
            self.queue_send_ting(&send, data).await?;
        }
        Ok(())
    }

    async fn queue_send_ting(&self, send: &SendRow, data: TingData) -> Result<()> {
        let s = send.clone();
        let isi = send.isi.clone();
        let subject = send.send_id.as_str().to_owned();
        self.db
            .tx(move |tx| queue_ting(tx, &s, &data, isi, &subject).map(|_| ()))
            .await?;
        self.outbox_wake.notify_one();
        Ok(())
    }

    /// `speech.done`.
    ///
    /// # Errors
    /// Database failures.
    pub async fn ui_speech_done(self: &SharedRef, op: SpeechDone) -> Result<()> {
        if op.stopped_by_user {
            self.speech.cancel(&op.send_id);
        }
        let sid = op.send_id.as_str().to_owned();
        let now = now_ms();
        let (send, first) = self
            .db
            .tx(move |tx| {
                let send = load_send(tx, &sid)?;
                let n = tx
                    .execute(
                        "UPDATE sends SET speech_done_at = ?2 WHERE send_id = ?1 AND speech_done_at IS NULL",
                        params![sid, now],
                    )
                    .sql()?;
                if n == 1 {
                    set_speech_outcome(tx, &sid, StoredSpeechStatus::Played)?;
                }
                Ok((send, n == 1))
            })
            .await?;
        let Some(send) = send else {
            return Err(Error::invalid_input(format!(
                "no send {} exists",
                op.send_id
            )));
        };
        if first && send.notify.contains(&Notify::SpeechFinished) && send.payload.speak.is_some() {
            let data = TingData::SpeechFinished(SpeechFinished {
                schema: SchemaV1,
                send_id: send.send_id.clone(),
                stopped_by_user: op.stopped_by_user,
                played_ms: op.played_ms,
                total_ms: op.total_ms,
                finished_at: Timestamp::from_unix_ms(now),
                slot: send.slot,
                context: send.key.context.data_context(),
            });
            self.queue_send_ting(&send, data).await?;
        }
        // `speech.done` never closes a send nor advances the queue: a
        // speak-only bubble stays up 1.5 s after its speech and closes on the
        // UI's `shown.done(speech_done)` (its slide-back); the watchdog armed
        // at `shown` covers a lost `shown.done`.
        Ok(())
    }

    /// `shown.done`.
    ///
    /// # Errors
    /// Database failures.
    pub async fn ui_shown_done(self: &SharedRef, op: ShownDone) -> Result<()> {
        let reason = match op.reason {
            silicon_peek_client::ipc::ui::ShownReason::Auto => "auto",
            silicon_peek_client::ipc::ui::ShownReason::SpeechDone => "speech_done",
        };
        let sid = op.send_id.as_str().to_owned();
        let ask = self.db.call(move |c| ask_of_send(c, &sid)).await?;
        if ask.is_some_and(|a| a.state == AskState::Pending) {
            // An ask ends only by answer, dismissal, expiry or cancel.
            tracing::debug!(send = %op.send_id, "ignoring shown.done for a pending ask");
            return Ok(());
        }
        self.close_send(&op.send_id, reason).await.map(|_| ())
    }

    /// The Silicon in a physical slot: production first (it has physical
    /// priority, gap-testing §9.3), else the most recent testing registration.
    ///
    /// # Errors
    /// `side_not_registered` when nobody holds it.
    pub async fn slot_owner(&self, slot: SlotIndex) -> Result<(ActorKey, HomeRef)> {
        self.slot_owner_in(slot, None).await
    }

    /// The Silicon holding `slot` in `context` (the additive `context` of
    /// `voice.submit`, `message` and `focus`), or [`Shared::slot_owner`]'s
    /// rule when the UI did not say.
    ///
    /// # Errors
    /// `side_not_registered` when nobody holds it (in that context).
    pub async fn slot_owner_in(
        &self,
        slot: SlotIndex,
        context: Option<Context>,
    ) -> Result<(ActorKey, HomeRef)> {
        let wanted = context.map(|c| c.as_string());
        let found: Option<(String, String, String, String, String)> = self
            .db
            .call(move |c| {
                c.query_row(
                    "SELECT context, org_id, actor_id, home_path, api_url FROM slots
                     WHERE slot = ?1 AND (?2 IS NULL OR context = ?2)
                     ORDER BY (context = 'production') DESC, registered_at DESC LIMIT 1",
                    params![slot.get(), wanted],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                )
                .optional()
                .sql()
            })
            .await?;
        let (context, org, actor, home_path, api_url) = found.ok_or_else(|| {
            Error::new(
                ErrorCode::SideNotRegistered,
                format!("no Silicon holds position {slot}"),
            )
        })?;
        let context = Context::parse(&context).map_err(|e| corrupt("slot context", e))?;
        Ok((
            ActorKey {
                context,
                org: OrgId::parse(&org).map_err(|e| corrupt("org", e))?,
                actor: ActorId::parse(&actor).map_err(|e| corrupt("actor", e))?,
            },
            HomeRef {
                home_path,
                api_url: ApiUrl::parse(&api_url).map_err(|e| corrupt("api url", e))?,
                context,
            },
        ))
    }

    /// Creates a Carbon message (§1.9.7) and queues its ting.
    ///
    /// # Errors
    /// `invalid_input` for empty text; `side_not_registered`.
    pub async fn create_message(
        self: &SharedRef,
        key: &ActorKey,
        home: &HomeRef,
        message_id: MessageId,
        text: &str,
        via: MessageVia,
    ) -> Result<MessageId> {
        let text = text.trim();
        if text.is_empty() {
            return Err(Error::invalid_input(
                "a Carbon message must not be empty; an empty answer is never sent",
            ));
        }
        if text.chars().count() > limits::TEXT_ANSWER_MAX_LENGTH as usize {
            return Err(Error::new(
                ErrorCode::TextTooLong,
                format!(
                    "a Carbon message is at most {} characters",
                    limits::TEXT_ANSWER_MAX_LENGTH
                ),
            ));
        }
        let k = key.clone();
        let (latest, slot): (Option<(String, Option<String>)>, Option<SlotIndex>) = self
            .db
            .call(move |c| {
                let latest = c
                    .query_row(
                        "SELECT send_id, isi FROM sends WHERE context = ?1 AND org_id = ?2 AND actor_id = ?3
                         ORDER BY created_at DESC, send_id DESC LIMIT 1",
                        params![k.context_str(), k.org.as_str(), k.actor.as_str()],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()
                    .sql()?;
                Ok((latest, slot_of(c, &k)?))
            })
            .await?;
        let slot = slot.ok_or_else(|| side_not_registered(&key.actor, &[]))?;
        let in_reply_to = latest.as_ref().and_then(|(s, _)| SendId::parse(s).ok());
        let isi = latest.and_then(|(_, isi)| isi);
        let now = now_ms();
        let data = TingData::MessageReceived(MessageReceived {
            schema: SchemaV1,
            message_id: message_id.clone(),
            text: text.to_owned(),
            via,
            sent_at: Timestamp::from_unix_ms(now),
            slot,
            context: key.context.data_context(),
            in_reply_to,
        });
        let req = silicon_peek_client::ting::DeliveryRequest::new(
            &key.actor,
            &data,
            silicon_peek_client::ting::TingMetadata::new(isi),
        )?;
        let bytes = req.to_bytes()?;
        let row = outbox::NewRow {
            event_id: req.event_id.as_str().to_owned(),
            key: key.clone(),
            home: home.clone(),
            kind: outbox::Kind::Ting,
            request: bytes,
            subject_id: Some(message_id.as_str().to_owned()),
        };
        self.db.tx(move |tx| outbox::insert(tx, &row)).await?;
        self.outbox_wake.notify_one();
        let mut rec = Record::new("message.received", "ok")
            .with("slot", slot.get())
            .with("input", enum_str(&via));
        rec.actor = Some((key.org.clone(), key.actor.clone()));
        rec.testing = key.context.is_testing();
        self.record(rec);
        Ok(message_id)
    }

    /// `message` (typed, no pending ask).
    ///
    /// # Errors
    /// As [`Shared::create_message`].
    pub async fn ui_message(self: &SharedRef, op: MessageOp) -> Result<MessageResult> {
        let (key, home) = self.slot_owner_in(op.slot, op.context).await?;
        let id = self
            .create_message(&key, &home, MessageId::generate(), &op.text, op.via)
            .await?;
        Ok(MessageResult { message_id: id })
    }

    // ----------------------------------------------------- CLI queries

    /// `ask.get` / `ask.list` rows.
    pub(crate) fn ask_info(c: &Connection, ask: &AskRow) -> Result<AskInfo> {
        let send = load_send(c, ask.send_id.as_str())?;
        let delivery = if ask.delivered_via.as_deref() == Some("wait") {
            Some(DeliveryInfo {
                status: DeliveryState::Wait,
                ting_id: None,
                silent: None,
            })
        } else if let Some(ev) = &ask.event_id {
            c.query_row(
                "SELECT status, ting_id, silent FROM outbox WHERE event_id = ?1",
                [ev],
                |r| {
                    let status: String = r.get(0)?;
                    let ting_id: Option<String> = r.get(1)?;
                    let silent: Option<i64> = r.get(2)?;
                    Ok((status, ting_id, silent))
                },
            )
            .optional()
            .sql()?
            .map(|(status, ting_id, silent)| DeliveryInfo {
                status: parse_enum(&status).unwrap_or(DeliveryState::Pending),
                ting_id,
                silent: silent.map(|s| s != 0),
            })
        } else {
            None
        };
        Ok(AskInfo {
            ask_id: ask.ask_id.clone(),
            state: ask.state,
            answer: ask.answer.clone(),
            via: ask.via,
            answered_at: ask.answered_at.map(Timestamp::from_unix_ms),
            delivery,
            send_id: Some(ask.send_id.clone()),
            question: send
                .as_ref()
                .and_then(|s| s.payload.ask.as_ref().map(|a| a.question.clone())),
            ask_type: send
                .as_ref()
                .and_then(|s| s.payload.ask.as_ref().map(Ask::ask_type)),
            transcript: ask.transcript.clone(),
            created_at: Some(Timestamp::from_unix_ms(ask.created_at)),
            expires_at: ask.expires_at.map(Timestamp::from_unix_ms),
        })
    }

    /// Asks of a Silicon, newest first; `states` filters (empty: all).
    ///
    /// # Errors
    /// Database failures.
    pub async fn list_asks(
        &self,
        key: &ActorKey,
        states: &[AskState],
        limit: u32,
    ) -> Result<Vec<AskInfo>> {
        let k = key.clone();
        let filter = (!states.is_empty()).then(|| {
            Value::Array(states.iter().map(|s| Value::String(enum_str(s))).collect()).to_string()
        });
        self.db
            .call(move |c| {
                let mut st = c
                    .prepare(&format!(
                        "SELECT {} FROM asks a JOIN sends s ON s.send_id = a.send_id
                         WHERE s.context = ?1 AND s.org_id = ?2 AND s.actor_id = ?3
                           AND (?4 IS NULL OR a.state IN (SELECT value FROM json_each(?4)))
                         ORDER BY a.created_at DESC, a.ask_id DESC LIMIT ?5",
                        ASK_COLS
                            .split(", ")
                            .map(|c| format!("a.{c}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                    .sql()?;
                let rows = st
                    .query_map(
                        params![
                            k.context_str(),
                            k.org.as_str(),
                            k.actor.as_str(),
                            filter,
                            limit
                        ],
                        ask_from_row,
                    )
                    .sql()?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .sql()?;
                rows.into_iter()
                    .map(|r| r.and_then(|a| Self::ask_info(c, &a)))
                    .collect()
            })
            .await
    }

    /// One ask of a Silicon.
    ///
    /// # Errors
    /// `ask_not_found` when it is not this Silicon's.
    pub async fn get_ask(&self, key: &ActorKey, ask_id: &AskId) -> Result<AskInfo> {
        let k = key.clone();
        let aid = ask_id.as_str().to_owned();
        let found = self
            .db
            .call(move |c| {
                let Some(ask) = load_ask(c, &aid)? else {
                    return Ok(None);
                };
                let Some(send) = load_send(c, ask.send_id.as_str())? else {
                    return Ok(None);
                };
                if send.key != k {
                    return Ok(None);
                }
                Self::ask_info(c, &ask).map(Some)
            })
            .await?;
        found.ok_or_else(|| {
            Error::new(
                ErrorCode::AskNotFound,
                format!("no ask {ask_id} belongs to {} in this context", key.actor),
            )
            .with_hint("list this Silicon's asks with `peek ask list`")
        })
    }

    /// Sends of a Silicon waiting behind its current one (waiting plus
    /// overflow).
    pub async fn queued_count(&self, key: &ActorKey) -> u32 {
        let core = self.core.lock().await;
        core.queues
            .get(key)
            .map_or(0, crate::state::ActorQueue::waiting_count)
    }

    /// Queues of every Silicon (for `app.update.prepare` decisions).
    pub async fn any_ask_on_screen(&self) -> bool {
        let core = self.core.lock().await;
        core.queues.values().any(|q| {
            q.current
                .as_ref()
                .is_some_and(|b| b.ask_id.is_some() && b.pushed)
        })
    }

    /// The current send and every waiting one (waiting, then overflow) in
    /// order, for tests and diagnostics.
    pub async fn snapshot_queue(&self, key: &ActorKey) -> (Option<SendId>, VecDeque<SendId>) {
        let core = self.core.lock().await;
        core.queues.get(key).map_or((None, VecDeque::new()), |q| {
            (
                q.current.as_ref().map(|b| b.send_id.clone()),
                q.waiting
                    .iter()
                    .chain(q.overflow.iter())
                    .map(|b| b.send_id.clone())
                    .collect(),
            )
        })
    }

    /// Pre-warms a Silicon's session and STT token (`focus`, ask shown).
    pub fn prewarm(self: &SharedRef, home: HomeRef, key: ActorKey) {
        let now = Instant::now();
        {
            let Ok(mut map) = self.prewarmed.lock() else {
                return;
            };
            if map
                .get(&home.home_path)
                .is_some_and(|t| now.duration_since(*t) < self.cfg.timings.prewarm_cooldown)
            {
                return;
            }
            map.insert(home.home_path.clone(), now);
        }
        let this = Arc::clone(self);
        tokio::spawn(async move {
            let policy = silicon_peek_client::runtime::RefreshPolicy::with_delays(
                this.cfg.timings.refresh_retry.clone(),
            );
            match this
                .net
                .session(&home, silicon_peek_client::runtime::PREWARM_MARGIN, &policy)
                .await
            {
                Ok(_) => {
                    if let Err(e) = this
                        .speech_token(
                            &home,
                            &key,
                            silicon_peek_client::api::SpeechPurpose::Stt,
                            false,
                        )
                        .await
                    {
                        tracing::debug!(error = %e, "pre-warming the STT token failed");
                    }
                }
                Err(e) => tracing::debug!(error = %e, "pre-warming a session failed"),
            }
        });
    }
}

/// Stores image bytes under `cache/images/<sha256>.<ext>` and returns the
/// absolute path.
///
/// # Errors
/// `image_unsupported`; I/O failures.
pub fn cache_image(dir: &std::path::Path, bytes: &[u8]) -> Result<String> {
    let format = ImageFormat::sniff(bytes).ok_or_else(|| {
        Error::new(
            ErrorCode::ImageUnsupported,
            "an image blob is not PNG, JPEG, HEIC, WebP or GIF",
        )
    })?;
    let name = format!("{}.{}", sha256_hex(bytes), format.extension());
    let path = dir.join(&name);
    if path.exists() {
        // A re-sent image is fresh again for the cache's age limit.
        if let Ok(f) = std::fs::File::options().write(true).open(&path) {
            let _ = f.set_modified(std::time::SystemTime::now());
        }
    } else {
        silicon_peek_client::runtime::fs::write_atomic(dir, &name, bytes)?;
    }
    Ok(path.to_string_lossy().into_owned())
}

/// Cached images no open send references are removed once older than this
/// (the Silicon's own file may be gone, so a bubble keeps its copy until it
/// closes; history never shows images).
pub const IMAGE_CACHE_MAX_AGE: Duration = Duration::from_hours(7 * 24);
/// The unreferenced images left are trimmed, oldest first, to this size.
pub const IMAGE_CACHE_MAX_BYTES: u64 = 500 * 1024 * 1024;
/// How often peekd prunes `cache/images/`.
pub const IMAGE_CACHE_PRUNE_EVERY: Duration = Duration::from_hours(6);

impl Shared {
    /// Prunes `cache/images/`: never an image an open send (queued, showing,
    /// or with a pending ask) or a scheduled send references; unreferenced
    /// images older than
    /// [`IMAGE_CACHE_MAX_AGE`], then the oldest unreferenced ones beyond
    /// [`IMAGE_CACHE_MAX_BYTES`], plus abandoned temp files.
    pub async fn prune_image_cache(&self) {
        let referenced = self
            .db
            .call(|c| {
                let mut st = c
                    .prepare(
                        "SELECT payload FROM sends WHERE closed_at IS NULL
                            OR send_id IN (SELECT send_id FROM asks WHERE state = 'pending')
                         UNION ALL SELECT payload FROM scheduled",
                    )
                    .sql()?;
                let rows = st
                    .query_map([], |r| r.get::<_, Vec<u8>>(0))
                    .sql()?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .sql()?;
                Ok(rows
                    .into_iter()
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
                    .collect::<Vec<String>>())
            })
            .await;
        let Ok(referenced) = referenced else {
            // Without knowing what is in use, nothing is removed.
            return;
        };
        let dir = self.paths.images_dir();
        let removed = tokio::task::spawn_blocking(move || {
            prune_images(
                &dir,
                &referenced,
                IMAGE_CACHE_MAX_AGE,
                IMAGE_CACHE_MAX_BYTES,
            )
        })
        .await
        .unwrap_or(0);
        if removed > 0 {
            tracing::info!(removed, "pruned cached images");
        }
    }
}

/// Removes files from an image cache directory that no `referenced` payload
/// names: temp files older than an hour, images older than `max_age`, then
/// the oldest images until the unreferenced ones fit in `max_bytes`.
/// Returns how many files were removed.
#[must_use]
pub fn prune_images(
    dir: &std::path::Path,
    referenced: &[String],
    max_age: Duration,
    max_bytes: u64,
) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    let now = std::time::SystemTime::now();
    let mut removed = 0;
    let mut kept: Vec<(std::path::PathBuf, std::time::SystemTime, u64)> = Vec::new();
    for e in rd.flatten() {
        let Ok(meta) = e.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        let mtime = meta.modified().unwrap_or(now);
        let age = now.duration_since(mtime).unwrap_or_default();
        if name.starts_with('.') {
            let temp = std::path::Path::new(&name)
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("tmp"));
            if temp && age > Duration::from_hours(1) {
                removed += usize::from(std::fs::remove_file(e.path()).is_ok());
            }
            continue;
        }
        if referenced.iter().any(|p| p.contains(&name)) {
            continue;
        }
        if age > max_age {
            removed += usize::from(std::fs::remove_file(e.path()).is_ok());
            continue;
        }
        kept.push((e.path(), mtime, meta.len()));
    }
    let mut total: u64 = kept.iter().map(|k| k.2).sum();
    kept.sort_by_key(|k| k.1);
    for (path, _, len) in kept {
        if total <= max_bytes {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            removed += 1;
            total = total.saturating_sub(len);
        }
    }
    removed
}

#[cfg(test)]
mod summary_tests {
    use super::*;

    fn payload(v: Value) -> SendPayload {
        serde_json::from_value(v).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn summaries_follow_the_contract() {
        let question = Ask::from_input(&json!({"question": "Delete  old.zip?", "type": "text"}))
            .unwrap_or_else(|e| panic!("{e}"));
        let ask = payload(json!({"ask": question, "speak": "x"}));
        assert_eq!(ask.summary(), "Delete old.zip?");
        let text = payload(json!({"show": {"elements": [
            {"type": "image", "path": "/c/a.png", "caption": "cover"},
            {"type": "text", "text": "Build\nfinished"}]}, "speak": "spoken"}));
        assert_eq!(
            text.summary(),
            "Build finished",
            "the first text wins over a caption"
        );
        let caption = payload(json!({"show": {"elements": [
            {"type": "image", "path": "/c/a.png"},
            {"type": "image", "path": "/c/b.png", "caption": " CO2 "}]}}));
        assert_eq!(caption.summary(), "CO2");
        let image = payload(json!({"show": {"elements": [{"type": "image", "path": "/c/a.png"}]}}));
        assert_eq!(image.summary(), "image");
        let speak = payload(json!({"speak": "  Deploy\t done  "}));
        assert_eq!(speak.summary(), "Deploy done");
        let long = payload(json!({"speak": "é".repeat(61)}));
        let s = long.summary();
        assert_eq!(s.chars().count(), 60);
        assert!(s.ends_with('…'));
        let exact = payload(json!({"speak": "a".repeat(60)}));
        assert_eq!(exact.summary(), "a".repeat(60));
        assert_eq!(payload(json!({})).summary(), "");
    }
}

#[cfg(test)]
mod image_cache_tests {
    use super::*;

    fn file(dir: &std::path::Path, name: &str, len: usize, age: Duration) {
        let path = dir.join(name);
        std::fs::write(&path, vec![0u8; len]).unwrap_or_else(|e| panic!("{e}"));
        let f = std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap_or_else(|e| panic!("{e}"));
        f.set_modified(std::time::SystemTime::now() - age)
            .unwrap_or_else(|e| panic!("{e}"));
    }

    #[test]
    fn unreferenced_images_are_pruned_by_age_and_size() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let d = dir.path();
        let day = Duration::from_hours(24);
        file(d, "aaa.png", 10, 30 * day); // referenced by an open send: kept
        file(d, "bbb.png", 10, 30 * day); // old, unreferenced: removed
        file(d, "ccc.jpg", 10, day); // fresh: kept
        file(d, ".ddd.png.0192.tmp", 10, 2 * day); // abandoned temp: removed
        file(d, ".eee.png.0193.tmp", 10, Duration::from_secs(5)); // being written: kept
        let open = vec![format!(
            r#"{{"show":{{"elements":[{{"type":"image","path":"{}"}}]}}}}"#,
            d.join("aaa.png").display()
        )];
        assert_eq!(prune_images(d, &open, IMAGE_CACHE_MAX_AGE, u64::MAX), 2);
        let left = |n: &str| d.join(n).exists();
        assert!(left("aaa.png") && left("ccc.jpg") && left(".eee.png.0193.tmp"));
        assert!(!left("bbb.png") && !left(".ddd.png.0192.tmp"));

        // Over the size cap: the oldest unreferenced go first, never a
        // referenced one.
        file(d, "fff.png", 100, 3 * day);
        file(d, "ggg.png", 100, 2 * day);
        file(d, "hhh.png", 100, day);
        assert_eq!(prune_images(d, &open, IMAGE_CACHE_MAX_AGE, 100), 3);
        assert!(left("aaa.png") && left("hhh.png"));
        assert!(!left("fff.png") && !left("ggg.png") && !left("ccc.jpg"));
    }

    #[test]
    fn a_re_sent_image_is_fresh_again() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        let path = cache_image(dir.path(), &png).unwrap_or_else(|e| panic!("{e}"));
        let old = std::time::SystemTime::now() - Duration::from_hours(30 * 24);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .and_then(|f| f.set_modified(old))
            .unwrap_or_else(|e| panic!("{e}"));
        cache_image(dir.path(), &png).unwrap_or_else(|e| panic!("{e}"));
        let modified = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .unwrap_or_else(|e| panic!("{e}"));
        let age = modified.elapsed().unwrap_or_default();
        assert!(age < Duration::from_secs(60));
        assert_eq!(
            prune_images(dir.path(), &[], IMAGE_CACHE_MAX_AGE, u64::MAX),
            0
        );
    }
}
