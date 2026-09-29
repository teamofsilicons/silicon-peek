//! Decode Google's streamed audio on the Mac; the backend only relays SSE.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use silicon_peek_client::{Error, ErrorCode, Result};

const MAX_EVENT_BYTES: usize = 2 * 1024 * 1024;

fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::SpeechUnavailable, message).with_retryable(true)
}

/// Incremental SSE decoder for Gemini Interactions audio deltas.
#[derive(Default)]
pub(crate) struct AudioStream {
    pending: Vec<u8>,
    data: Vec<u8>,
    completed: bool,
}

impl AudioStream {
    /// Accept arbitrary HTTP chunks, returning only decoded PCM audio.
    pub(crate) fn push(&mut self, chunk: &[u8]) -> Result<Vec<u8>> {
        let mut audio = Vec::new();
        for part in chunk.split_inclusive(|b| *b == b'\n') {
            if self.pending.len() + part.len() > MAX_EVENT_BYTES {
                return Err(invalid("Gemini sent an oversized stream event"));
            }
            self.pending.extend_from_slice(part);
            if self.pending.last() != Some(&b'\n') {
                continue;
            }
            self.pending.pop();
            if self.pending.last() == Some(&b'\r') {
                self.pending.pop();
            }
            if self.pending.is_empty() {
                self.event(&mut audio)?;
            } else if let Some(data) = self.pending.strip_prefix(b"data:") {
                let data = data.strip_prefix(b" ").unwrap_or(data);
                if self.data.len() + data.len() + 1 > MAX_EVENT_BYTES {
                    return Err(invalid("Gemini sent an oversized stream event"));
                }
                self.data.extend_from_slice(data);
                self.data.push(b'\n');
            }
            self.pending.clear();
        }
        Ok(audio)
    }

    fn event(&mut self, audio: &mut Vec<u8>) -> Result<()> {
        let data = std::mem::take(&mut self.data);
        let data = data.trim_ascii();
        if data.is_empty() || data == b"[DONE]" {
            return Ok(());
        }
        let event: serde_json::Value = serde_json::from_slice(data)
            .map_err(|_| invalid("Gemini sent an invalid stream event"))?;
        match event.get("event_type").and_then(serde_json::Value::as_str) {
            Some("interaction.completed") => self.completed = true,
            Some("error" | "interaction.failed" | "interaction.cancelled") => {
                return Err(invalid("Gemini could not complete speech generation"));
            }
            Some("interaction.status_update")
                if matches!(
                    event["status"].as_str(),
                    Some("failed" | "cancelled" | "incomplete")
                ) =>
            {
                return Err(invalid("Gemini could not complete speech generation"));
            }
            Some("step.delta") if event["delta"]["type"] == "audio" => {
                if self.completed {
                    return Err(invalid("Gemini sent audio after stream completion"));
                }
                let delta = &event["delta"];
                if let Some(mime) = delta["mime_type"].as_str()
                    && !matches!(
                        mime.split(';').next().map(str::trim),
                        Some("audio/l16" | "audio/pcm")
                    )
                {
                    return Err(invalid("Gemini returned an unsupported audio format"));
                }
                let encoded = delta["data"]
                    .as_str()
                    .ok_or_else(|| invalid("Gemini audio was missing its data"))?;
                let decoded = STANDARD
                    .decode(encoded)
                    .map_err(|_| invalid("Gemini returned invalid base64 audio"))?;
                audio.extend(decoded);
            }
            _ => {}
        }
        Ok(())
    }

    /// An interrupted stream must never become a successful cache entry.
    pub(crate) fn finish(&self) -> Result<()> {
        if self.completed
            && self.pending.trim_ascii().is_empty()
            && self.data.trim_ascii().is_empty()
        {
            Ok(())
        } else {
            Err(invalid("Gemini's audio stream ended before completion"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragmented_sse_audio_completion_and_failures() -> Result<()> {
        let wire = b": heartbeat\r\nevent: step.delta\r\ndata: {";
        let rest = b"\"event_type\":\"step.delta\",\"delta\":{\"type\":\"audio\",\"data\":\"AAECAw==\"}}\r\n\r\ndata: {\"event_type\":\"interaction.completed\"}\n\ndata: [DONE]\n\n";
        let mut decoder = AudioStream::default();
        let mut audio = Vec::new();
        for byte in wire.iter().chain(rest) {
            audio.extend(decoder.push(&[*byte])?);
        }
        assert_eq!(audio, [0, 1, 2, 3]);
        decoder.finish()?;
        assert!(AudioStream::default().finish().is_err());
        assert!(
            AudioStream::default()
                .push(b"data: {\"event_type\":\"error\"}\n\n")
                .is_err()
        );
        assert!(AudioStream::default().push(b"data: {\"event_type\":\"step.delta\",\"delta\":{\"type\":\"audio\",\"data\":\"??\"}}\n\n").is_err());
        assert!(
            AudioStream::default()
                .push(&vec![b'x'; MAX_EVENT_BYTES + 1])
                .is_err()
        );
        Ok(())
    }
}
