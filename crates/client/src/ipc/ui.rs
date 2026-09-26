//! peekd ↔ Peek.app messages (BLUEPRINT §1.6 "UI ops and events").
//!
//! Peek.app holds no tokens and does no networking: peekd pushes what to show
//! and streams TTS audio; the app reports what the Carbon did. There is no
//! live transcript anywhere (D9): speech-to-text runs once, after recording
//! stops, and its outcome arrives as [`SttResult`].

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Empty, EventBody, Op};
use crate::{
    api::TestingEnvironment,
    error::ErrorObject,
    identity::{ActorId, Context, OrgId, SlotIndex},
    ids::{AskId, MessageId, SendId},
    ipc::cli::{DrawingStats, SpeechStatus, Warning},
    schema::{ask::Ask, show::Show},
    timestamp::Timestamp,
    ting::{Gesture, MessageVia},
};

// ---------------------------------------------------------------- events →UI

/// A registered drawing, as the UI loads it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotDrawing {
    /// Hex SHA-256.
    pub sha256: String,
    /// Absolute path under `~/Library/Application Support/Peek/drawings/`.
    pub path: String,
}

/// One occupied slot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotState {
    /// 1..=8.
    pub index: SlotIndex,
    /// The data context.
    pub context: Context,
    /// The Silicon.
    pub actor_id: ActorId,
    /// Its org.
    pub org_id: OrgId,
    /// Its display name, when known.
    #[serde(default)]
    pub display_name: Option<String>,
    /// The initial drawn by the fallback visual.
    pub initial: String,
    /// The active drawing.
    #[serde(default)]
    pub drawing: Option<SlotDrawing>,
    /// Whether the Carbon hotkey is registered.
    pub hotkey: bool,
    /// The testing environment `{id, name, generation}`, for the
    /// `TEST · <name>` pill and its tooltip (additive; Peek.app reads
    /// `environment`, older peekd builds sent `testing`).
    #[serde(default, alias = "testing", skip_serializing_if = "Option::is_none")]
    pub environment: Option<TestingEnvironment>,
}

/// `slots.state`: the full slot table (sent on connect and on every change).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotsState {
    /// Every occupied slot, across contexts.
    pub slots: Vec<SlotState>,
}

impl EventBody for SlotsState {
    const NAME: &'static str = "slots.state";
}

/// The speak part of a bubble.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeakInfo {
    /// The text (shown as a pill when speech is unavailable).
    pub text: String,
    /// TTS status.
    pub status: SpeechStatus,
}

/// `peek.show`: slide a bubble in. Image paths are absolute cache paths.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PeekShow {
    /// The send.
    pub send_id: SendId,
    /// The slot.
    pub slot: SlotIndex,
    /// The data context.
    pub context: Context,
    /// Speech, with `--speak`.
    #[serde(default)]
    pub speak: Option<SpeakInfo>,
    /// Content, with `--show`.
    #[serde(default)]
    pub show: Option<Show>,
    /// The question, with `--ask`.
    #[serde(default)]
    pub ask: Option<Ask>,
    /// The ask's ID (echoed back in `answer` and `voice.submit`).
    #[serde(default)]
    pub ask_id: Option<AskId>,
    /// How long a show stays up without speech, or after it.
    #[serde(default)]
    pub duration_ms: Option<u64>,
    /// Sends queued behind this one in the slot.
    #[serde(default)]
    pub queued_behind: u32,
    /// When the ask expires, if it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Timestamp>,
}

impl EventBody for PeekShow {
    const NAME: &'static str = "peek.show";
}

/// `tts.begin`: a new PCM stream for a send.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TtsBegin {
    /// The send.
    pub send_id: SendId,
    /// Always `s16le`.
    pub format: String,
    /// Always 24000.
    pub sample_rate: u32,
    /// Always 1.
    pub channels: u16,
    /// Estimated total frames (`chars ÷ 14 × 24000`).
    pub est_frames: u64,
}

impl TtsBegin {
    /// The Aura-2 stream format peekd requests.
    #[must_use]
    pub fn aura(send_id: SendId, est_frames: u64) -> Self {
        Self {
            send_id,
            format: "s16le".to_owned(),
            sample_rate: 24_000,
            channels: 1,
            est_frames,
        }
    }
}

impl EventBody for TtsBegin {
    const NAME: &'static str = "tts.begin";
}

/// `tts.chunk`: one blob of PCM (≤ 64 KiB), in order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TtsChunk {
    /// The send.
    pub send_id: SendId,
    /// 0-based sequence number.
    pub seq: u64,
}

impl EventBody for TtsChunk {
    const NAME: &'static str = "tts.chunk";
}

/// `tts.end`: the stream is complete.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TtsEnd {
    /// The send.
    pub send_id: SendId,
    /// Exact total frames.
    pub total_frames: u64,
}

impl EventBody for TtsEnd {
    const NAME: &'static str = "tts.end";
}

/// `tts.error`: TTS failed before any audio played (after retries).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TtsError {
    /// The send.
    pub send_id: SendId,
    /// Why.
    pub error: ErrorObject,
}

impl EventBody for TtsError {
    const NAME: &'static str = "tts.error";
}

/// Why a bubble is withdrawn by peekd.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelReason {
    /// `peek ask cancel`.
    CancelledBySilicon,
    /// `peek unregister` or `peek logout`.
    Unregistered,
    /// `--expires-in` elapsed.
    Expired,
}

/// `peek.cancel`: slide a bubble out.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeekCancel {
    /// The send.
    pub send_id: SendId,
    /// Why.
    pub reason: CancelReason,
}

impl EventBody for PeekCancel {
    const NAME: &'static str = "peek.cancel";
}

/// The outcome of one transcription.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SttOutcome {
    /// The transcript answered the ask (or became the message).
    Matched,
    /// The transcript matched no option; the bubble stays open.
    Unmatched,
    /// Nothing was said; the bubble stays open.
    Empty,
    /// Transcription failed after retries; the UI offers typing.
    Failed,
}

/// `stt.result`: the single, final transcription outcome.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SttResult {
    /// The ask, for a voice answer.
    #[serde(default)]
    pub ask_id: Option<AskId>,
    /// The message, for a Carbon message.
    #[serde(default)]
    pub message_id: Option<MessageId>,
    /// The outcome.
    pub outcome: SttOutcome,
    /// The resolved answer value (or transcript), when matched.
    #[serde(default)]
    pub value: Option<Value>,
    /// The failure, when failed.
    #[serde(default)]
    pub error: Option<ErrorObject>,
}

impl EventBody for SttResult {
    const NAME: &'static str = "stt.result";
}

/// `restarting`: peekd is about to restart the app on a new build.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Restarting {
    /// The build being installed.
    pub to_build: u64,
}

impl EventBody for Restarting {
    const NAME: &'static str = "restarting";
}

// ------------------------------------------------------- requests peekd→UI

/// `drawing.validate`: run visual.md A9 validation in a scratch runtime.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DrawingValidate {
    /// The script to validate (a temp path peekd wrote).
    pub script_path: String,
    /// Return a PNG preview blob.
    pub preview: bool,
    /// Return this test frame's op stream.
    pub dump_frame: Option<u32>,
}

/// A validation failure.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DrawingValidationError {
    /// The exception message.
    pub message: String,
    /// The JS stack.
    #[serde(default)]
    pub stack: Option<String>,
    /// The test frame that failed.
    #[serde(default)]
    pub frame: Option<u32>,
    /// A summary of that frame's `input`.
    #[serde(default)]
    pub input_summary: Option<Value>,
}

/// `drawing.validate` result (plus a PNG blob with `preview`). Peek.app sends
/// `"stats": null` when the script never ran a frame (load failure); that
/// reads as zero statistics.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DrawingValidateResult {
    /// Whether the drawing passed.
    pub ok: bool,
    /// Test-frame statistics.
    #[serde(default, deserialize_with = "null_as_default")]
    pub stats: DrawingStats,
    /// Warnings.
    #[serde(default)]
    pub warnings: Vec<Warning>,
    /// `peek.log` output.
    #[serde(default)]
    pub logs: Vec<Value>,
    /// The failure, when not ok.
    #[serde(default)]
    pub error: Option<DrawingValidationError>,
    /// The dumped frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dump: Option<Value>,
}

fn null_as_default<'de, D, T>(d: D) -> std::result::Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

impl Op for DrawingValidate {
    const NAME: &'static str = "drawing.validate";
    type Output = DrawingValidateResult;
}

/// `drawing.load`: activate a validated drawing for a slot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DrawingLoad {
    /// The data context.
    pub context: Context,
    /// The org.
    pub org_id: OrgId,
    /// The Silicon.
    pub actor_id: ActorId,
    /// Its slot.
    pub slot: SlotIndex,
    /// The activated script.
    pub script_path: String,
    /// Its hash.
    pub sha256: String,
}

/// `drawing.load` result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OkResult {
    /// Whether it worked.
    pub ok: bool,
}

impl Op for DrawingLoad {
    const NAME: &'static str = "drawing.load";
    type Output = OkResult;
}

/// `app.update.prepare`: may the app restart onto `build` now?
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppUpdatePrepare {
    /// The new build.
    pub build: u64,
}

/// `app.update.prepare` / `app.quit` result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadyResult {
    /// True only with no bubble visible, no recording and no ask on screen.
    pub ready: bool,
}

impl Op for AppUpdatePrepare {
    const NAME: &'static str = "app.update.prepare";
    type Output = ReadyResult;
}

/// `app.quit`: quit for an update to `build`, if idle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppQuit {
    /// The new build.
    pub build: u64,
}

impl Op for AppQuit {
    const NAME: &'static str = "app.quit";
    type Output = ReadyResult;
}

/// `app.uninstall` (peekd → UI, relayed from `peek app uninstall`):
/// unregister the login item and the agent, move the bundle to the Trash,
/// then quit. The result is any JSON, e.g. `{"accepted":true}`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiAppUninstall {}

impl Op for UiAppUninstall {
    const NAME: &'static str = "app.uninstall";
    type Output = Value;
}

/// `doctor` (peekd → UI, relayed from `peek doctor`): what only Peek.app
/// knows. Result (read leniently):
/// `{"mic":"granted|denied|restricted|undetermined","hotkeys":{"registered":[…],"failed":[…]},…}`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiDoctor {}

impl Op for UiDoctor {
    const NAME: &'static str = "doctor";
    type Output = Value;
}

// ------------------------------------------------------- requests UI→peekd

/// How a Carbon answered in the UI (voice answers use `voice.submit`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiAnswerVia {
    /// Clicked.
    Click,
    /// Typed.
    Keyboard,
}

/// `answer`: the raw value is resolved by peekd with
/// [`Ask::resolve_answer`](crate::schema::ask::Ask::resolve_answer):
/// text → string, single choice → option id, multiple choice → array of ids,
/// slider → number, range → `[from, to]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnswerOp {
    /// The send.
    pub send_id: SendId,
    /// The ask.
    pub ask_id: AskId,
    /// The raw value.
    pub value: Value,
    /// Click or keyboard.
    pub via: UiAnswerVia,
}

impl Op for AnswerOp {
    const NAME: &'static str = "answer";
    type Output = Empty;
}

/// `voice.submit`: one blob, a 16 kHz mono 16-bit WAV (≤ 4 MiB, ≤ 120 s). A
/// voice answer when `ask_id` is set, otherwise a Carbon message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceSubmit {
    /// The send whose ask is answered.
    pub send_id: Option<SendId>,
    /// The ask, for a voice answer.
    pub ask_id: Option<AskId>,
    /// The slot.
    pub slot: SlotIndex,
    /// Recording length.
    pub duration_ms: u64,
    /// `Locale.preferredLanguages`, for STT language selection.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<String>,
    /// The slot's data context (`production` or an environment UUID), so a
    /// test-partition registration of a physical slot is addressed exactly.
    /// Absent means "whoever holds the slot, production first".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Context>,
}

/// `voice.submit` result. For a Carbon voice message (`ask_id` null) it
/// names the new message, so the UI matches the later [`SttResult`] by
/// `message_id` instead of by arrival order; `null` for a voice answer (its
/// `stt.result` carries the `ask_id`). peekd starts transcribing only after
/// this reply is queued, so the `stt.result` never overtakes it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceSubmitResult {
    /// The new message, for a voice message.
    #[serde(default)]
    pub message_id: Option<MessageId>,
}

impl Op for VoiceSubmit {
    const NAME: &'static str = "voice.submit";
    type Output = VoiceSubmitResult;
}

/// `ui.status` (UI → peekd, additive): what only Peek.app knows, pushed after
/// every successful `hello` and whenever a value changes. peekd keeps the
/// latest report of the connected app and uses it for `peek doctor` when the
/// live `doctor` relay cannot be answered. Read leniently: every field is
/// optional and unknown fields are kept as sent.
///
/// ```json
/// {"mic":"granted","hotkeys":{"modifier":"ctrl+cmd","registered":["ctrl+cmd+1"],"failed":[],"problems":[]},
///  "glass":"live","services":"enabled","app_build":1000,"app_version":"0.1.0"}
/// ```
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UiStatusReport {
    /// `granted`, `denied`, `restricted` or `undetermined`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mic: Option<String>,
    /// `{"modifier","registered":[…],"failed":[…],"problems":[…]}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hotkeys: Option<Value>,
    /// Everything else the app reports (`glass`, `services`, `app_build`, …).
    #[serde(flatten)]
    pub other: serde_json::Map<String, Value>,
}

impl Op for UiStatusReport {
    const NAME: &'static str = "ui.status";
    type Output = Empty;
}

/// Why the Carbon can or cannot see bubbles.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresenceReason {
    /// The screen is on and unlocked.
    #[default]
    Ok,
    /// The screen is locked (or the session is inactive, e.g. fast user
    /// switching).
    Locked,
    /// The displays are asleep.
    Asleep,
    /// No display is on.
    DisplayOff,
    /// A reason this peekd does not know (a newer app); read as sent by the
    /// `available` flag alone.
    #[serde(other)]
    Other,
}

/// `presence` (UI → peekd, additive): whether the Carbon can see bubbles.
/// Peek.app sends it right after its `hello` and on every change (screen
/// locked/unlocked, displays asleep/awake, session resigned/active).
///
/// While `available` is false peekd pushes no `peek.show` and starts no
/// speech: new sends wait in their Silicon's queue (status `queued`, warning
/// `carbon_away`) and asks keep their expiry clock. When it turns true again
/// the queued sends are shown in order. An app that never sends `presence`
/// is treated as available.
///
/// ```json
/// {"available":false,"reason":"locked"}
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Presence {
    /// Whether bubbles can be seen.
    pub available: bool,
    /// Why.
    #[serde(default)]
    pub reason: PresenceReason,
}

impl Default for Presence {
    fn default() -> Self {
        Self {
            available: true,
            reason: PresenceReason::Ok,
        }
    }
}

impl Op for Presence {
    const NAME: &'static str = "presence";
    type Output = Empty;
}

/// `message`: a typed Carbon message with no pending ask.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageOp {
    /// The slot.
    pub slot: SlotIndex,
    /// The text.
    pub text: String,
    /// Always `keyboard`.
    pub via: MessageVia,
    /// The slot's data context (see [`VoiceSubmit::context`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Context>,
}

/// `message` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageResult {
    /// The new message.
    pub message_id: MessageId,
}

impl Op for MessageOp {
    const NAME: &'static str = "message";
    type Output = MessageResult;
}

/// `dismissed`: the Carbon closed a bubble.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dismissed {
    /// The send.
    pub send_id: SendId,
    /// How.
    pub gesture: Gesture,
}

impl Op for Dismissed {
    const NAME: &'static str = "dismissed";
    type Output = Empty;
}

/// `speech.done`: playback finished or was stopped.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeechDone {
    /// The send.
    pub send_id: SendId,
    /// Stopped by a double click on the down arrow.
    pub stopped_by_user: bool,
    /// Milliseconds played.
    pub played_ms: u64,
    /// Total milliseconds.
    pub total_ms: u64,
}

impl Op for SpeechDone {
    const NAME: &'static str = "speech.done";
    type Output = Empty;
}

/// Why a show retracted on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShownReason {
    /// Its duration elapsed.
    Auto,
    /// Speech finished (plus 1.5 s).
    SpeechDone,
}

/// `shown.done`: a show retracted without the Carbon closing it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShownDone {
    /// The send.
    pub send_id: SendId,
    /// Milliseconds visible.
    pub visible_ms: u64,
    /// Why.
    pub reason: ShownReason,
}

impl Op for ShownDone {
    const NAME: &'static str = "shown.done";
    type Output = Empty;
}

/// `focus`: the Carbon summoned a slot; pre-warm its session and speech token.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Focus {
    /// The slot.
    pub slot: SlotIndex,
    /// The slot's data context (see [`VoiceSubmit::context`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Context>,
}

impl Op for Focus {
    const NAME: &'static str = "focus";
    type Output = Empty;
}

/// How a drawing failed at runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DrawingFailure {
    /// 10 throws in a row.
    Throws,
    /// 30 overruns in 5 s.
    Overruns,
    /// Exceeded 16 MiB.
    Oom,
}

/// `drawing.error`: the fallback visual replaced a drawing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DrawingError {
    /// The data context.
    pub context: Context,
    /// The org.
    pub org_id: OrgId,
    /// The Silicon.
    pub actor_id: ActorId,
    /// How it failed.
    pub reason: DrawingFailure,
    /// The last error message.
    pub message: String,
    /// Its stack.
    #[serde(default)]
    pub stack: Option<String>,
}

impl Op for DrawingError {
    const NAME: &'static str = "drawing.error";
    type Output = Empty;
}

/// `telemetry` from the UI (same shape as the CLI op).
pub type UiTelemetry = crate::ipc::cli::Telemetry;

/// `settings.changed`: mirrored into `settings.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SettingsChanged {
    /// The setting (e.g. `telemetry`, `voice_defaults`).
    pub key: String,
    /// Its new value.
    pub value: Value,
}

impl Op for SettingsChanged {
    const NAME: &'static str = "settings.changed";
    type Output = Empty;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        error::{Error, Result},
        ipc::{Event, Request},
    };
    use serde_json::json;

    #[test]
    fn tts_events() -> Result<()> {
        let id = SendId::generate();
        let begin = Event::new(&TtsBegin::aura(id.clone(), 48_000), vec![])?;
        assert_eq!(begin.fields["format"], "s16le");
        assert_eq!(begin.fields["sample_rate"], 24000);
        let chunk = Event::new(
            &TtsChunk {
                send_id: id.clone(),
                seq: 0,
            },
            vec![vec![0; 4]],
        )?;
        assert_eq!(chunk.event, "tts.chunk");
        assert_eq!(chunk.blobs.len(), 1);
        let end: TtsEnd = Event::new(
            &TtsEnd {
                send_id: id,
                total_frames: 9,
            },
            vec![],
        )?
        .parse()?;
        assert_eq!(end.total_frames, 9);
        Ok(())
    }

    #[test]
    fn no_live_transcript_events_exist() {
        for name in [
            SlotsState::NAME,
            PeekShow::NAME,
            TtsBegin::NAME,
            TtsChunk::NAME,
            TtsEnd::NAME,
            TtsError::NAME,
            PeekCancel::NAME,
            SttResult::NAME,
            Restarting::NAME,
        ] {
            assert!(
                !name.contains("word") && !name.contains("transcript"),
                "{name}"
            );
        }
    }

    #[test]
    fn ui_requests_round_trip() -> Result<()> {
        let answer = AnswerOp {
            send_id: SendId::generate(),
            ask_id: AskId::generate(),
            value: json!(["1", "3"]),
            via: UiAnswerVia::Click,
        };
        let req = Request::new(&answer, None, vec![])?;
        assert_eq!(req.op, "answer");
        assert_eq!(req.parse::<AnswerOp>()?, answer);
        let voice = VoiceSubmit {
            send_id: None,
            ask_id: None,
            slot: SlotIndex::new(2)?,
            duration_ms: 1200,
            languages: vec![],
            context: None,
        };
        let v = serde_json::to_value(&voice).map_err(|e| Error::internal(e.to_string()))?;
        assert_eq!(
            v,
            json!({"send_id":null,"ask_id":null,"slot":2,"duration_ms":1200})
        );
        let stt: SttResult = serde_json::from_value(json!({"ask_id":null,"message_id":null,"outcome":"failed","value":null,"error":{"code":"speech_unavailable","message":"m"}}))
            .map_err(|e| Error::internal(e.to_string()))?;
        assert_eq!(stt.outcome, SttOutcome::Failed);
        // The reply names a voice message; a voice answer's reply has null
        // (and an older peekd's empty reply still decodes).
        let message_id = MessageId::generate();
        let reply = serde_json::to_value(VoiceSubmitResult {
            message_id: Some(message_id.clone()),
        })
        .map_err(|e| Error::internal(e.to_string()))?;
        assert_eq!(reply, json!({"message_id": message_id.as_str()}));
        let old: VoiceSubmitResult =
            serde_json::from_value(json!({})).map_err(|e| Error::internal(e.to_string()))?;
        assert_eq!(old.message_id, None);
        Ok(())
    }

    #[test]
    fn presence_round_trips_and_reads_new_reasons() -> Result<()> {
        let e = |e: serde_json::Error| Error::internal(e.to_string());
        let p: Presence =
            serde_json::from_value(json!({"available": false, "reason": "display_off"}))
                .map_err(e)?;
        assert_eq!(
            p,
            Presence {
                available: false,
                reason: PresenceReason::DisplayOff
            }
        );
        assert_eq!(
            serde_json::to_value(Presence::default()).map_err(e)?,
            json!({"available": true, "reason": "ok"})
        );
        let newer: Presence =
            serde_json::from_value(json!({"available": false, "reason": "screensaver"}))
                .map_err(e)?;
        assert_eq!(newer.reason, PresenceReason::Other);
        let bare: Presence = serde_json::from_value(json!({"available": true})).map_err(e)?;
        assert_eq!(bare, Presence::default());
        let req = Request::new(&p, None, Vec::new())?;
        assert_eq!(req.op, "presence");
        Ok(())
    }

    #[test]
    fn ui_status_reports_are_read_leniently() -> Result<()> {
        let e = |x: serde_json::Error| Error::internal(x.to_string());
        let full = json!({"mic":"granted","hotkeys":{"modifier":"ctrl+cmd","registered":["ctrl+cmd+1"],
            "failed":["ctrl+cmd+3"],"problems":["taken"]},"glass":"live","services":"enabled",
            "app_build":1000,"app_version":"0.1.0"});
        let req = Request::new(
            &serde_json::from_value::<UiStatusReport>(full.clone()).map_err(e)?,
            None,
            vec![],
        )?;
        assert_eq!(req.op, "ui.status");
        let report = req.parse::<UiStatusReport>()?;
        assert_eq!(report.mic.as_deref(), Some("granted"));
        assert_eq!(report.other["app_build"], 1000);
        assert_eq!(serde_json::to_value(&report).map_err(e)?, full);
        let empty: UiStatusReport = serde_json::from_value(json!({})).map_err(e)?;
        assert_eq!(empty, UiStatusReport::default());
        Ok(())
    }

    #[test]
    fn additive_ui_fields_from_peek_app() -> Result<()> {
        let e = |x: serde_json::Error| Error::internal(x.to_string());
        let voice: VoiceSubmit = serde_json::from_value(json!({"send_id":null,"ask_id":null,"slot":3,
            "duration_ms":900,"context":"0192f2d2-7c9e-7cc0-8b2e-6f3a2b1c0d9e","languages":["en-IN","hi"]}))
        .map_err(e)?;
        assert!(voice.context.is_some_and(|c| c.is_testing()));
        let message: MessageOp =
            serde_json::from_value(json!({"slot":1,"text":"hi","via":"keyboard"})).map_err(e)?;
        assert!(message.context.is_none());
        let focus: Focus =
            serde_json::from_value(json!({"slot":1,"context":"production"})).map_err(e)?;
        assert_eq!(focus.context, Some(Context::Production));
        // The Swift ValidationReport encodes a missing `stats` as null.
        let report: DrawingValidateResult = serde_json::from_value(json!({"ok":false,"stats":null,"warnings":[],
            "logs":["x"],"error":{"message":"SyntaxError","stack":null,"frame":null,"input_summary":"phase=idle"}}))
        .map_err(e)?;
        assert_eq!(report.stats.frames, 0);
        assert_eq!(
            report.error.and_then(|x| x.input_summary),
            Some(json!("phase=idle"))
        );
        let slot: SlotState = serde_json::from_value(json!({"index":2,"context":"production","actor_id":"si:cleanup",
            "org_id":"tos","initial":"C","hotkey":true,"testing":{"id":"0192f2d2-7c9e-7cc0-8b2e-6f3a2b1c0d9e","name":"t","generation":1}}))
        .map_err(e)?;
        let out = serde_json::to_value(&slot).map_err(e)?;
        assert_eq!(out["environment"]["name"], "t");
        assert!(out.get("testing").is_none());
        Ok(())
    }
}
