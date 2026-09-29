//! Voice answers and voice messages (BLUEPRINT §1.9.5, §1.9.7): the WAV is
//! transcribed once after recording stops, using `OpenAI` through Peek.
//! There is no live transcript anywhere (D9). The recording stays in
//! `recordings/` until its answer is delivered or its row ends.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use serde_json::Value;
use silicon_peek_client::{
    Error, ErrorCode, Result,
    api::{SpeechPurpose, SpeechTranscript},
    identity::SlotIndex,
    ids::{AskId, MessageId},
    ipc::{
        cli::AskState,
        ui::{SttOutcome, SttResult, VoiceSubmit, VoiceSubmitResult},
    },
    schema::{
        ask::{Ask, AskKind},
        limits,
    },
    ting::{AnswerVia, MessageVia},
};

use crate::{
    bubbles::{Resolution, load_ask, load_send},
    matching::{MatchOutcome, match_transcript},
    net::HomeRef,
    speech::{jitter, millis},
    speech_request::{ListenParams, RetryBudget, RetryKind, SpeechError, SttLanguage, keyterms},
    state::{ActorKey, Shared, SharedRef},
    telemetry::Record,
};

/// Recordings whose loudest 20 ms window is below this count as "nothing
/// heard" (§1.9.5: −50 dBFS).
pub const SILENCE_DBFS: f64 = -50.0;

/// A validated 16-bit PCM mono WAV.
#[derive(Clone, Debug, PartialEq)]
pub struct Wav {
    /// Samples per second.
    pub sample_rate: u32,
    /// Duration.
    pub duration: Duration,
    /// Loudest 20 ms window, dBFS.
    pub peak_dbfs: f64,
}

fn le16(b: &[u8], at: usize) -> Option<u16> {
    b.get(at..at + 2).map(|s| u16::from_le_bytes([s[0], s[1]]))
}

fn le32(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// Parses and checks a recording: RIFF/WAVE, PCM, mono, 16-bit, ≤ 120 s.
///
/// # Errors
/// `invalid_input` naming what is wrong.
pub fn parse_wav(bytes: &[u8]) -> Result<Wav> {
    let bad =
        |why: &str| Error::invalid_input(format!("the voice recording is not a usable WAV: {why}"));
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(bad("it has no RIFF/WAVE header"));
    }
    let mut pos = 12usize;
    let mut fmt: Option<(u16, u16, u32, u16)> = None;
    let mut data: Option<&[u8]> = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = usize::try_from(le32(bytes, pos + 4).unwrap_or(0)).unwrap_or(usize::MAX);
        let body_start = pos + 8;
        let body_end = body_start.saturating_add(len).min(bytes.len());
        let body = &bytes[body_start..body_end];
        match id {
            b"fmt " => {
                fmt = Some((
                    le16(body, 0).ok_or_else(|| bad("the fmt chunk is truncated"))?,
                    le16(body, 2).ok_or_else(|| bad("the fmt chunk is truncated"))?,
                    le32(body, 4).ok_or_else(|| bad("the fmt chunk is truncated"))?,
                    le16(body, 14).ok_or_else(|| bad("the fmt chunk is truncated"))?,
                ));
            }
            b"data" => {
                data = Some(body);
                break;
            }
            _ => {}
        }
        pos = body_start.saturating_add(len).saturating_add(len % 2);
    }
    let (format, channels, rate, bits) = fmt.ok_or_else(|| bad("it has no fmt chunk"))?;
    let data = data.ok_or_else(|| bad("it has no data chunk"))?;
    if format != 1 || bits != 16 {
        return Err(bad(&format!(
            "it is format {format} with {bits}-bit samples; peek needs 16-bit PCM"
        )));
    }
    if channels != 1 {
        return Err(bad(&format!(
            "it has {channels} channels; peek records mono"
        )));
    }
    if !(8000..=48_000).contains(&rate) {
        return Err(bad(&format!(
            "its sample rate {rate} Hz is outside 8–48 kHz"
        )));
    }
    let samples = data.len() / 2;
    let secs = f64::from(u32::try_from(samples).unwrap_or(u32::MAX)) / f64::from(rate);
    if secs > f64::from(limits::RECORDING_MAX_SECONDS) + 0.5 {
        return Err(Error::invalid_input(format!(
            "the voice recording is {secs:.1} s long; the limit is {} s",
            limits::RECORDING_MAX_SECONDS
        )));
    }
    let window = usize::try_from(rate / 50).unwrap_or(320).max(1);
    let mut peak = 0.0f64;
    for chunk in data.chunks(window * 2) {
        let n = chunk.len() / 2;
        if n == 0 {
            continue;
        }
        let sum: f64 = chunk
            .as_chunks::<2>()
            .0
            .iter()
            .map(|s| {
                let v = f64::from(i16::from_le_bytes(*s));
                v * v
            })
            .sum();
        let rms = (sum / f64::from(u32::try_from(n).unwrap_or(1))).sqrt();
        peak = peak.max(rms);
    }
    let peak_dbfs = if peak <= 0.0 {
        f64::NEG_INFINITY
    } else {
        20.0 * (peak / 32768.0).log10()
    };
    Ok(Wav {
        sample_rate: rate,
        duration: Duration::from_secs_f64(secs),
        peak_dbfs,
    })
}

/// STT language (notes/speech §6): a fixed setting wins; `auto` uses the
/// Mac's preferred languages (one → `language`, several → `detect_language`).
#[must_use]
pub fn stt_language(setting: &str, preferred: &[String]) -> SttLanguage {
    if setting != "auto" && !setting.is_empty() {
        return SttLanguage::Fixed(setting.to_ascii_lowercase());
    }
    let mut langs: Vec<String> = Vec::new();
    for p in preferred {
        let primary = p
            .split(['-', '_'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        if (2..=3).contains(&primary.len())
            && primary.bytes().all(|b| b.is_ascii_lowercase())
            && !langs.contains(&primary)
        {
            langs.push(primary);
        }
    }
    langs.truncate(5);
    match langs.len() {
        0 => SttLanguage::DetectAny,
        1 => SttLanguage::Fixed(langs.remove(0)),
        _ => SttLanguage::Detect(langs),
    }
}

/// What a recording answers.
#[derive(Clone, Debug)]
struct VoiceJob {
    target: Target,
    key: ActorKey,
    home: HomeRef,
    wav: Vec<u8>,
    info: Wav,
    languages: Vec<String>,
}

#[derive(Clone, Debug)]
enum Target {
    Ask { ask_id: AskId, ask: Box<Ask> },
    Message { message_id: MessageId },
}

/// A transcription accepted by `voice.submit`. The UI connection starts it
/// only after the `voice.submit` reply is queued, so the `stt.result` (which
/// can be immediate, e.g. `empty` for silence) never overtakes the reply
/// that names its `message_id`.
#[derive(Debug)]
pub struct SttJob {
    job: VoiceJob,
    slot: SlotIndex,
}

impl SttJob {
    /// Transcribes in the background.
    pub fn start(self, shared: &SharedRef) {
        let this = Arc::clone(shared);
        tokio::spawn(async move { this.run_stt(self.job, self.slot).await });
    }
}

impl Shared {
    /// `voice.submit`: checks the WAV, resolves its target and keeps the
    /// recording. The returned [`SttJob`] transcribes in the background once
    /// started (the UI shows `transcribing` until `stt.result`); the result
    /// names the new message for a voice message.
    ///
    /// # Errors
    /// `invalid_input` for a bad WAV or blob count; `ask_not_found`;
    /// `side_not_registered` for a message to an empty slot.
    pub async fn ui_voice_submit(
        self: &SharedRef,
        op: VoiceSubmit,
        blobs: Vec<Vec<u8>>,
    ) -> Result<(VoiceSubmitResult, SttJob)> {
        let [wav] = <[Vec<u8>; 1]>::try_from(blobs).map_err(|b| {
            Error::invalid_input(format!(
                "voice.submit carries exactly one WAV blob, not {}",
                b.len()
            ))
        })?;
        if wav.len() > limits::WAV_MAX_BYTES {
            return Err(Error::new(
                ErrorCode::FrameTooLarge,
                format!(
                    "the recording is {} bytes; the limit is {}",
                    wav.len(),
                    limits::WAV_MAX_BYTES
                ),
            ));
        }
        let info = parse_wav(&wav)?;
        let (target, key, home) = self.voice_target(&op).await?;
        let id = match &target {
            Target::Ask { ask_id, .. } => ask_id.as_str().to_owned(),
            Target::Message { message_id } => message_id.as_str().to_owned(),
        };
        silicon_peek_client::runtime::fs::write_atomic(
            &self.paths.recordings_dir(),
            &format!("{id}.wav"),
            &wav,
        )?;
        let result = VoiceSubmitResult {
            message_id: match &target {
                Target::Ask { .. } => None,
                Target::Message { message_id } => Some(message_id.clone()),
            },
        };
        let job = VoiceJob {
            target,
            key,
            home,
            wav,
            info,
            languages: op.languages,
        };
        Ok((result, SttJob { job, slot: op.slot }))
    }

    /// What a recording answers: a pending ask (checked against its send),
    /// or a new Carbon message to the slot's Silicon.
    async fn voice_target(&self, op: &VoiceSubmit) -> Result<(Target, ActorKey, HomeRef)> {
        let Some(ask_id) = &op.ask_id else {
            let (key, home) = self.slot_owner_in(op.slot, op.context).await?;
            return Ok((
                Target::Message {
                    message_id: MessageId::generate(),
                },
                key,
                home,
            ));
        };
        let aid = ask_id.as_str().to_owned();
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
                format!("no ask {ask_id} exists"),
            ));
        };
        if ask.state != AskState::Pending {
            return Err(Error::new(
                ErrorCode::AskNotFound,
                format!("ask {ask_id} is no longer pending"),
            ));
        }
        if op.send_id.as_ref().is_some_and(|s| *s != ask.send_id) {
            return Err(Error::invalid_input(format!(
                "ask {ask_id} belongs to send {}, not the one given",
                ask.send_id
            )));
        }
        let question = send.payload.ask.clone().ok_or_else(|| {
            Error::internal(format!(
                "send {} has an ask row but no question",
                send.send_id
            ))
        })?;
        Ok((
            Target::Ask {
                ask_id: ask_id.clone(),
                ask: Box::new(question),
            },
            send.key.clone(),
            send.home.clone(),
        ))
    }

    fn stt_event(
        &self,
        target: &Target,
        outcome: SttOutcome,
        value: Option<Value>,
        error: Option<&Error>,
    ) {
        let (ask_id, message_id) = match target {
            Target::Ask { ask_id, .. } => (Some(ask_id.clone()), None),
            Target::Message { message_id } => (None, Some(message_id.clone())),
        };
        self.ui.event(
            &SttResult {
                ask_id,
                message_id,
                outcome,
                value,
                error: error.map(Error::to_object),
            },
            Vec::new(),
        );
    }

    async fn run_stt(self: SharedRef, job: VoiceJob, slot: SlotIndex) {
        let started = Instant::now();
        let recording_id = match &job.target {
            Target::Ask { ask_id, .. } => ask_id.as_str().to_owned(),
            Target::Message { message_id } => message_id.as_str().to_owned(),
        };
        let mut rec = Record::new("stt.request", "ok").with("slot", slot.get());
        rec.actor = Some((job.key.org.clone(), job.key.actor.clone()));
        rec.testing = job.key.context.is_testing();
        if job.info.peak_dbfs < SILENCE_DBFS {
            self.stt_event(&job.target, SttOutcome::Empty, None, None);
            let _ = std::fs::remove_file(self.paths.recording(&recording_id));
            self.record(rec.with("status", "silent").with("matched", false));
            return;
        }
        let (numerals, labels): (bool, Vec<String>) = match &job.target {
            Target::Ask { ask, .. } => (
                matches!(ask.kind, AskKind::Slider { .. } | AskKind::Range { .. }),
                ask.options().iter().map(|o| o.label.clone()).collect(),
            ),
            Target::Message { .. } => (false, Vec::new()),
        };
        let params = ListenParams {
            numerals,
            keyterms: keyterms(labels.iter().map(String::as_str)),
            language: stt_language(&self.settings.get().stt_language, &job.languages),
        };
        let transcript = match self.transcribe(&job, &params).await {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(error = %e, "transcription failed");
                self.stt_event(&job.target, SttOutcome::Failed, None, Some(&e));
                if matches!(job.target, Target::Message { .. }) {
                    // Nothing will ever reference a failed message's audio.
                    let _ = std::fs::remove_file(self.paths.recording(&recording_id));
                }
                rec.outcome = "error";
                rec.error_code = Some(e.code().to_string());
                rec.duration_ms = Some(millis(started.elapsed()));
                self.record(rec.with("status", "failed"));
                return;
            }
        };
        rec = rec.with("stt_ms", millis(started.elapsed()));
        if let Some(l) = &transcript.detected_language {
            rec = rec.with("stt_language", l.clone());
        }
        if let Some(r) = &transcript.request_id {
            rec = rec.with("stt_request_id", r.clone());
        }
        let text = transcript.text.trim().to_owned();
        let matched = match &job.target {
            Target::Message { message_id } => {
                self.finish_message(&job, message_id, &text, &recording_id)
                    .await
            }
            Target::Ask { ask_id, ask } => {
                self.finish_answer(&job.target, ask_id, ask, &text).await
            }
        };
        self.record(rec.with("matched", matched));
    }

    /// A voice message: the transcript becomes `peek.message.received`.
    async fn finish_message(
        self: &SharedRef,
        job: &VoiceJob,
        message_id: &MessageId,
        text: &str,
        recording_id: &str,
    ) -> bool {
        if text.is_empty() {
            self.stt_event(&job.target, SttOutcome::Empty, None, None);
            let _ = std::fs::remove_file(self.paths.recording(recording_id));
            return false;
        }
        let msg: String = text
            .chars()
            .take(limits::TEXT_ANSWER_MAX_LENGTH as usize)
            .collect();
        self.stt_event(
            &job.target,
            SttOutcome::Matched,
            Some(Value::String(msg.clone())),
            None,
        );
        if let Err(e) = self
            .create_message(
                &job.key,
                &job.home,
                message_id.clone(),
                &msg,
                MessageVia::Voice,
            )
            .await
        {
            tracing::warn!(error = %e, "recording a voice message failed");
        }
        true
    }

    /// A voice answer: match the transcript; only a match answers the ask
    /// (an empty answer is never sent).
    async fn finish_answer(
        self: &SharedRef,
        target: &Target,
        ask_id: &AskId,
        ask: &Ask,
        text: &str,
    ) -> bool {
        let outcome = match_transcript(ask, text);
        let resolved = match &outcome {
            MatchOutcome::Matched(v) => ask.resolve_answer(v).ok().map(|a| (v.clone(), a)),
            _ => None,
        };
        match (outcome, resolved) {
            (MatchOutcome::Empty, _) => {
                self.stt_event(target, SttOutcome::Empty, None, None);
                false
            }
            (_, Some((value, answer))) => {
                self.stt_event(target, SttOutcome::Matched, Some(value), None);
                let r = self
                    .resolve_ask(
                        ask_id,
                        Resolution::Answered {
                            answer,
                            via: AnswerVia::Voice,
                            transcript: Some(text.to_owned()),
                        },
                    )
                    .await;
                if let Err(e) = r {
                    tracing::info!(ask = %ask_id, error = %e, "a voice answer arrived after the ask closed");
                }
                true
            }
            _ => {
                self.stt_event(target, SttOutcome::Unmatched, None, None);
                false
            }
        }
    }

    /// Completed-audio transcription, with retries and one Peek session
    /// refresh on 401, all within the STT budget.
    async fn transcribe(&self, job: &VoiceJob, params: &ListenParams) -> Result<SpeechTranscript> {
        let mut budget = RetryBudget::new(
            Instant::now() + self.cfg.timings.stt_budget,
            &self.cfg.timings.stt_retry,
        );
        let mut fresh_session = false;
        let expired = || {
            Error::new(
                ErrorCode::SpeechUnavailable,
                format!(
                    "no transcription within the {} s budget",
                    self.cfg.timings.stt_budget.as_secs()
                ),
            )
            .with_retryable(true)
        };
        loop {
            // Minting spends the same STT budget (a stalled backend fails
            // into "Couldn't transcribe" instead of after 30 s).
            let minted = tokio::time::timeout(
                budget.remaining(),
                self.speech_token(&job.home, &job.key, SpeechPurpose::Stt, false),
            )
            .await;
            let Ok(minted) = minted else {
                return Err(expired());
            };
            let token = match minted {
                Ok(t) => t,
                Err(e) => match budget.next(if e.retryable() {
                    RetryKind::Retry
                } else {
                    RetryKind::Fatal
                }) {
                    Some(d) => {
                        tokio::time::sleep(jitter(d)).await;
                        continue;
                    }
                    None => return Err(e),
                },
            };
            let remaining = budget.remaining();
            if remaining.is_zero() {
                return Err(expired());
            }
            let attempted = tokio::time::timeout(
                remaining,
                self.listen_via(
                    &job.home,
                    &job.key,
                    &token,
                    params,
                    job.wav.clone(),
                    remaining,
                    std::mem::take(&mut fresh_session),
                ),
            )
            .await;
            let Ok(attempted) = attempted else {
                return Err(expired());
            };
            match attempted {
                Ok(t) => return Ok(t),
                Err(SpeechError {
                    kind: RetryKind::Reauth,
                    error,
                    ..
                }) => {
                    if !budget.reauth() {
                        return Err(error);
                    }
                    fresh_session = true;
                }
                Err(SpeechError {
                    kind,
                    error,
                    retry_after,
                }) => match budget.next(kind) {
                    Some(d) => {
                        tokio::time::sleep(
                            retry_after
                                .unwrap_or_else(|| jitter(d))
                                .min(budget.remaining()),
                        )
                        .await;
                    }
                    None => return Err(error),
                },
            }
        }
    }
}

/// A WAV for tests and tools: 16-bit mono PCM at `rate` Hz.
#[must_use]
pub fn wav_bytes(samples: &[i16], rate: u32) -> Vec<u8> {
    let data_len = u32::try_from(samples.len() * 2).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// A 440 Hz test tone of `millis` ms at 16 kHz and `amplitude` (0–32767).
#[must_use]
pub fn tone(millis: u32, amplitude: i16) -> Vec<i16> {
    let n = millis * 16;
    (0..n)
        .map(|i| {
            let t = f64::from(i) / 16_000.0;
            #[allow(clippy::cast_possible_truncation)] // |sin| ≤ 1, so bounded by amplitude
            let v = (f64::from(amplitude) * (t * 440.0 * std::f64::consts::TAU).sin()) as i16;
            v
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_checks_and_silence() -> Result<()> {
        let loud = parse_wav(&wav_bytes(&tone(1000, 8000), 16_000))?;
        assert!(loud.peak_dbfs > -20.0);
        assert!((loud.duration.as_secs_f64() - 1.0).abs() < 0.01);
        let quiet = parse_wav(&wav_bytes(&tone(1000, 50), 16_000))?;
        assert!(quiet.peak_dbfs < SILENCE_DBFS);
        let silent = parse_wav(&wav_bytes(&vec![0; 16_000], 16_000))?;
        assert!(silent.peak_dbfs.is_infinite());
        assert!(parse_wav(b"RIFF....WAVE").is_err());
        assert!(parse_wav(b"not a wav").is_err());
        let mut stereo = wav_bytes(&tone(100, 8000), 16_000);
        stereo[22] = 2;
        assert!(parse_wav(&stereo).is_err());
        let long = wav_bytes(&vec![1; 16_000 * 121], 16_000);
        assert!(parse_wav(&long).is_err());
        Ok(())
    }

    #[test]
    fn language_selection() {
        assert_eq!(
            stt_language("de", &["en-US".into()]),
            SttLanguage::Fixed("de".into())
        );
        assert_eq!(
            stt_language("auto", &["en-IN".into()]),
            SttLanguage::Fixed("en".into())
        );
        assert_eq!(
            stt_language("auto", &["en-IN".into(), "hi-IN".into(), "en-GB".into()]),
            SttLanguage::Detect(vec!["en".into(), "hi".into()])
        );
        assert_eq!(stt_language("auto", &[]), SttLanguage::DetectAny);
    }
}
