//! Direct, speech-only Voice Agent audio using a short-lived Deepgram JWT.

use std::time::Duration;

use bytes::Bytes;
use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use silicon_peek_client::{
    Error, ErrorCode, Result, Secret, api::SpeechSpeakRequest, identity::ApiUrl,
};
use tokio::{
    net::TcpStream,
    time::{Instant, Interval, interval_at, timeout_at},
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async_with_config,
    tungstenite::{
        Error as SocketError, Message, client::IntoClientRequest as _, protocol::WebSocketConfig,
    },
};

use crate::speech_request::{SpeechError, classify_proxy_error};

pub(crate) const MODEL: &str = "eleven_v4";
const TOTAL_TIMEOUT: Duration = Duration::from_secs(180);
const AUDIO_MAX_BYTES: usize = 20 * 1024 * 1024;

fn unavailable(message: &str) -> Error {
    Error::new(ErrorCode::SpeechUnavailable, message).with_retryable(true)
}

fn rejected(message: &str) -> Error {
    Error::new(ErrorCode::SpeechUnavailable, message).with_retryable(false)
}

/// JWTs go only to the Voice Agent endpoint, or a loopback test paired with a local backend.
pub(crate) fn check_agent_url(endpoint: &str, api: &ApiUrl) -> Result<()> {
    let url = url::Url::parse(endpoint)
        .map_err(|_| rejected("the speech token contains an invalid Voice Agent URL"))?;
    let loopback = |u: &url::Url| match u.host() {
        Some(url::Host::Domain("localhost")) => true,
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        _ => false,
    };
    let canonical = url.scheme() == "wss"
        && url.host_str() == Some("agent.deepgram.com")
        && url.port_or_known_default() == Some(443)
        && url.path() == "/v1/agent/converse";
    let local = url.scheme() == "ws"
        && loopback(&url)
        && url::Url::parse(api.as_str()).is_ok_and(|api| loopback(&api));
    if (canonical || local)
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
    {
        Ok(())
    } else {
        Err(rejected(
            "the speech token must connect to Deepgram's secure Voice Agent endpoint",
        ))
    }
}

fn connection_error(error: SocketError) -> SpeechError {
    let error = match error {
        SocketError::Http(response) => {
            let status = response.status().as_u16();
            Error::new(
                ErrorCode::SpeechUnavailable,
                "Deepgram refused the speech connection",
            )
            .with_status(status)
            .with_retryable(status == 429 || status >= 500)
        }
        _ => unavailable("Deepgram's speech connection could not be opened"),
    };
    classify_proxy_error(error)
}

/// Keep short performances together; queue long narration at natural sentence boundaries.
fn sentences(text: &str) -> Vec<&str> {
    if text.chars().count() <= 300 {
        return vec![text];
    }
    let mut parts = Vec::new();
    let mut start = 0;
    let mut count = 0;
    let mut in_cue = false;
    let end_of_text = text.trim_end().len();
    let mut chars = text.char_indices().peekable();
    while let Some((index, c)) = chars.next() {
        count += 1;
        match c {
            '[' => in_cue = true,
            ']' => in_cue = false,
            _ => {}
        }
        let boundary = matches!(c, '\n' | '。' | '！' | '？')
            || (matches!(c, '.' | '!' | '?')
                && chars.peek().is_none_or(|(_, next)| next.is_whitespace()));
        let end = index + c.len_utf8();
        if count >= 90 && !in_cue && boundary && end < end_of_text {
            parts.push(&text[start..end]);
            start = end;
            count = 0;
        }
    }
    parts.push(&text[start..]);
    parts
}

fn messages(request: &SpeechSpeakRequest) -> Vec<String> {
    let cue = request
        .voice_instructions
        .as_deref()
        .unwrap_or_default()
        .replace(['[', ']'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    sentences(&request.text)
        .into_iter()
        .map(|text| {
            if cue.is_empty() {
                text.to_owned()
            } else {
                format!("[{cue}] {text}")
            }
        })
        .collect()
}

fn settings(request: &SpeechSpeakRequest) -> Value {
    let mut provider = json!({"type":"eleven_labs", "model_id":MODEL, "voice_id":request.model});
    if let Some(language) = &request.language {
        provider["language_code"] = json!(
            language
                .split(['-', '_'])
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase()
        );
    }
    json!({"type":"Settings", "mip_opt_out":true, "flags":{"history":false},
        "audio":{"input":{"encoding":"linear16","sample_rate":24000},
                 "output":{"encoding":"linear16","sample_rate":24000,"container":"none"}},
        "agent":{"speak":{"provider":provider}}})
}

/// Owns the socket: cancelling the speech task closes upstream work immediately.
pub(crate) struct Audio {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    pending_text: Option<Vec<String>>,
    expected_messages: usize,
    spoken_messages: usize,
    deadline: Instant,
    idle: Duration,
    idle_deadline: Instant,
    keepalive: Interval,
    bytes: usize,
    pub(crate) request_id: Option<String>,
}

impl Audio {
    pub(crate) async fn connect(
        endpoint: &str,
        token: &Secret,
        request: &SpeechSpeakRequest,
        first_byte: Duration,
        idle: Duration,
    ) -> std::result::Result<Self, SpeechError> {
        let started = Instant::now();
        let deadline = started + first_byte;
        let mut upgrade = endpoint.into_client_request().map_err(connection_error)?;
        let mut auth: tokio_tungstenite::tungstenite::http::HeaderValue =
            format!("Bearer {}", token.expose()).parse().map_err(|_| {
                classify_proxy_error(rejected("the temporary speech credential is invalid"))
            })?;
        auth.set_sensitive(true);
        upgrade.headers_mut().insert("authorization", auth);
        let config = WebSocketConfig::default()
            .max_message_size(Some(1024 * 1024))
            .max_frame_size(Some(1024 * 1024));
        let (mut socket, _) = timeout_at(
            deadline.min(started + Duration::from_secs(10)),
            connect_async_with_config(upgrade, Some(config), false),
        )
        .await
        .map_err(|_| classify_proxy_error(unavailable("the speech connection timed out")))?
        .map_err(connection_error)?;
        let request_id = timeout_at(deadline, async {
            loop {
                match socket.next().await {
                    Some(Ok(Message::Text(text))) => {
                        let event: Value = serde_json::from_str(&text)
                            .map_err(|_| unavailable("Deepgram returned an invalid welcome"))?;
                        if event["type"] != "Welcome" {
                            return Err(unavailable(
                                "Deepgram did not welcome the speech connection",
                            ));
                        }
                        return Ok(event["request_id"].as_str().map(str::to_owned));
                    }
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                    _ => {
                        return Err(unavailable(
                            "the speech connection closed before its welcome",
                        ));
                    }
                }
            }
        })
        .await
        .map_err(|_| classify_proxy_error(unavailable("the speech welcome timed out")))?
        .map_err(classify_proxy_error)?;
        timeout_at(
            deadline,
            socket.send(Message::Text(settings(request).to_string().into())),
        )
        .await
        .map_err(|_| classify_proxy_error(unavailable("the speech settings timed out")))?
        .map_err(connection_error)?;
        let messages = messages(request);
        Ok(Self {
            socket,
            expected_messages: messages.len(),
            spoken_messages: 0,
            pending_text: Some(messages),
            deadline: started + TOTAL_TIMEOUT,
            idle,
            idle_deadline: deadline,
            keepalive: interval_at(started + Duration::from_secs(4), Duration::from_secs(4)),
            bytes: 0,
            request_id,
        })
    }

    /// Only `AgentAudioDone` is successful EOF; close, refusal and partial audio fail.
    pub(crate) async fn next(&mut self) -> Result<Option<Bytes>> {
        loop {
            let deadline = self.deadline.min(self.idle_deadline);
            let incoming = tokio::select! {
                () = tokio::time::sleep_until(deadline) => return Err(unavailable("the speech stream timed out before completion")),
                _ = self.keepalive.tick() => {
                    timeout_at(deadline, self.socket.send(Message::Text(json!({"type":"KeepAlive"}).to_string().into())))
                        .await.map_err(|_| unavailable("the speech connection timed out"))?
                        .map_err(|_| unavailable("the speech connection failed"))?;
                    continue;
                }
                incoming = self.socket.next() => incoming,
            };
            match incoming {
                Some(Ok(Message::Binary(data))) if !data.is_empty() => {
                    self.bytes = self.bytes.saturating_add(data.len());
                    if self.pending_text.is_some() || self.bytes > AUDIO_MAX_BYTES {
                        return Err(unavailable("Deepgram returned unusable speech audio"));
                    }
                    self.idle_deadline = Instant::now() + self.idle;
                    return Ok(Some(data));
                }
                Some(Ok(Message::Text(text))) => {
                    let event: Value = serde_json::from_str(&text)
                        .map_err(|_| unavailable("Deepgram returned an invalid speech event"))?;
                    match event["type"].as_str() {
                        Some("SettingsApplied") => {
                            let messages = self.pending_text.take().ok_or_else(|| {
                                unavailable("Deepgram repeated its settings response")
                            })?;
                            for text in messages {
                                let injection = json!({"type":"InjectAgentMessage", "message":text, "behavior":"queue"});
                                timeout_at(
                                    deadline,
                                    self.socket
                                        .send(Message::Text(injection.to_string().into())),
                                )
                                .await
                                .map_err(|_| unavailable("the speech connection timed out"))?
                                .map_err(|_| unavailable("the speech connection failed"))?;
                            }
                        }
                        Some("ConversationText") if event["role"] == "assistant" => {
                            self.spoken_messages += 1;
                        }
                        Some("AgentAudioDone")
                            if self.bytes > 0
                                && self.expected_messages > 1
                                && self.spoken_messages < self.expected_messages => {}
                        Some("AgentAudioDone") if self.bytes > 0 => return Ok(None),
                        Some("AgentAudioDone") => {
                            return Err(unavailable("Deepgram completed without speech audio"));
                        }
                        Some("InjectionRefused") => {
                            return Err(rejected("Deepgram refused the speech injection"));
                        }
                        Some("Error") => {
                            return Err(match event["code"].as_str() {
                                Some("INVALID_SETTINGS" | "FAILED_TO_SPEAK") => rejected(
                                    "Deepgram refused the ElevenLabs voice or speech settings",
                                ),
                                _ => unavailable("Deepgram failed to complete the speech"),
                            });
                        }
                        // Slow-generation warnings are not failures. Never log provider descriptions.
                        _ => {}
                    }
                }
                Some(Ok(Message::Close(_))) | None => {
                    return Err(unavailable(
                        "the speech connection closed before audio completion",
                    ));
                }
                Some(Err(_)) => {
                    return Err(unavailable(
                        "the speech connection failed before audio completion",
                    ));
                }
                _ => {}
            }
        }
    }
}
