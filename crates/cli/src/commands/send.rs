//! `peek send` (BLUEPRINT §1.9.3, §1.9.6, §7.4).
//!
//! Everything is validated locally before peekd is contacted: flag
//! combinations, `--speak`, the `--show`/`--ask` JSON (strict: duplicate or
//! unknown keys refused), every limit, and every image, whose bytes the CLI
//! reads (relative to the current directory) and sends as frame blobs so
//! Peek.app never opens the Silicon's paths. With `--wait` the connection
//! stays open for the `ask.result` event; on timeout or disconnect the
//! answer falls back to Ting.

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use silicon_peek_client::{
    Error, ErrorCode, Result,
    ipc::cli::{AskResult, AskState, SendOp, SendResult},
    runtime::daemon::{EventWait, REQUEST_TIMEOUT},
    schema::{
        ImageRef,
        ask::Ask,
        send::{
            Notify, SendFlags, check_duration, check_expires_in, check_flags, check_isi,
            check_speak, check_voice, check_wait, normalize_language,
        },
        show::Show,
    },
};

use super::{mac_session, next};
use crate::{
    cli::SendArgs,
    context::Globals,
    input,
    output::Out,
    service::{self, require_mac},
};

/// A validated send: the op and its image blobs.
#[derive(Debug)]
pub struct Prepared {
    /// The IPC op.
    pub op: SendOp,
    /// One blob per image reference, in reference order.
    pub blobs: Vec<Vec<u8>>,
    /// `--wait` duration, when waiting.
    pub wait: Option<Duration>,
}

fn replace_images<'a>(
    images: impl Iterator<Item = &'a mut ImageRef>,
    blobs: &mut Vec<Vec<u8>>,
) -> Result<()> {
    for image in images {
        if let ImageRef::Path(path) = image {
            let bytes = input::read_image(path)?;
            *image = ImageRef::Blob { blob: blobs.len() };
            blobs.push(bytes);
        }
    }
    Ok(())
}

/// Validates the arguments and reads the images (no peekd, no network).
pub fn prepare(args: &SendArgs, default_notify: &[Notify]) -> Result<Prepared> {
    check_flags(SendFlags {
        speak: args.speak.is_some(),
        show: args.show.is_some(),
        ask: args.ask.is_some(),
        voice: args.voice.is_some(),
        lang: args.lang.is_some(),
        duration: args.duration.is_some(),
        expires_in: args.expires_in.is_some(),
        wait: args.wait.is_some(),
    })?;
    if let Some(speak) = &args.speak {
        check_speak(speak)?;
    }
    let mut show = args
        .show
        .as_deref()
        .map(|raw| input::read_json("--show", raw).and_then(|v| Show::from_input(&v)))
        .transpose()?;
    let mut ask = args
        .ask
        .as_deref()
        .map(|raw| input::read_json("--ask", raw).and_then(|v| Ask::from_input(&v)))
        .transpose()?;
    if let Some(v) = &args.voice {
        check_voice(v)?;
    }
    let lang = args.lang.as_deref().map(normalize_language).transpose()?;
    let duration_ms = args
        .duration
        .map(|s| check_duration(s).map(|d| d.as_secs() * 1000))
        .transpose()?;
    if let Some(s) = args.expires_in {
        check_expires_in(s)?;
    }
    let wait = args.wait.map(|s| check_wait(Some(s))).transpose()?;
    let notify = match &args.notify {
        Some(list) => Notify::parse_list(list)?,
        None => default_notify.to_vec(),
    };
    let isi = match std::env::var("ISI") {
        Ok(v) if !v.is_empty() => {
            check_isi(&v).map_err(|e| {
                Error::invalid_input(format!(
                    "the ISI environment variable is invalid: {}",
                    e.message()
                ))
                .with_hint(
                    "ISI is optional context (at most 160 characters, one line); fix or unset it",
                )
            })?;
            Some(v)
        }
        _ => None,
    };
    let mut blobs = Vec::new();
    if let Some(show) = show.as_mut() {
        replace_images(show.images_mut(), &mut blobs)?;
    }
    if let Some(ask) = ask.as_mut() {
        replace_images(ask.images_mut(), &mut blobs)?;
    }
    let op = SendOp {
        isi,
        speak: args.speak.clone(),
        show,
        ask,
        voice: args.voice.clone(),
        lang,
        notify,
        duration_ms,
        expires_in_s: args.expires_in,
        wait: wait.is_some(),
    };
    // The same check peekd runs, so a mismatch is caught here first.
    op.validate(&blobs)?;
    Ok(Prepared { op, blobs, wait })
}

pub async fn run(g: &Globals, out: Out, args: SendArgs) -> Result<()> {
    require_mac("send")?;
    g.check_stdin(&[
        ("--show", args.show.as_deref() == Some("-")),
        ("--ask", args.ask.as_deref() == Some("-")),
    ])?;
    // Validate before touching the session so input errors are deterministic.
    let default_notify = crate::context::existing_store()
        .ok()
        .flatten()
        .and_then(|s| s.read_config().ok())
        .map(|c| c.notify)
        .unwrap_or_default();
    let prepared = prepare(&args, &default_notify)?;
    let (_session, auth) = mac_session(g).await?;
    let mut svc = service::ensure_service().await?;
    let started = Instant::now();
    let (result, _) = svc
        .call(&prepared.op, Some(&auth), prepared.blobs, REQUEST_TIMEOUT)
        .await?;
    out.warnings(&result.warnings);
    let Some(wait) = prepared.wait else {
        out.result(&result, human_send);
        let mut hints = Vec::new();
        if let Some(ask) = &result.ask_id {
            hints.push(format!("peek ask get {ask}"));
            hints.push(
                "answers arrive as Ting events peek.ask.answered (peek docs ting)".to_owned(),
            );
        } else {
            hints.push("peek history".to_owned());
        }
        let refs: Vec<&str> = hints.iter().map(String::as_str).collect();
        next(out, &refs);
        return Ok(());
    };
    let ask_id = result.ask_id.clone().ok_or_else(|| {
        Error::new(
            ErrorCode::UnexpectedResponse,
            "peekd accepted a --wait send with an ask but returned no ask_id",
        )
        .with_hint("update Peek.app (peek app update); report it with peek report if it persists")
    })?;
    let value = wait_for_answer(&mut svc, &result, &ask_id, wait, started).await?;
    out.value(&value, human_wait);
    if value["state"] == "pending" {
        next(out, &[&format!("peek ask get {ask_id}"), "peek docs ting"]);
    }
    Ok(())
}

async fn wait_for_answer(
    svc: &mut service::Service,
    sent: &SendResult,
    ask_id: &silicon_peek_client::ids::AskId,
    wait: Duration,
    started: Instant,
) -> Result<Value> {
    let deadline = started + wait;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match svc.next_event(remaining).await? {
            EventWait::Event(e) if e.event == "ask.result" => {
                let r: AskResult = e.parse()?;
                if &r.ask_id != ask_id {
                    continue;
                }
                // Only an acknowledged result counts as delivered here; if
                // the ack cannot be written peekd also sends the ting (at
                // least once), and the answer is printed either way.
                let _ = svc.acknowledge_result(ask_id).await;
                return Ok(final_value(&r, sent));
            }
            EventWait::Event(_) => {}
            EventWait::Closed | EventWait::TimedOut => break,
        }
    }
    Ok(json!({"ask_id": ask_id, "state": "pending", "delivery": "ting", "send_id": sent.send_id}))
}

fn final_value(r: &AskResult, sent: &SendResult) -> Value {
    let mut v = json!({"ask_id": r.ask_id, "state": r.state, "send_id": sent.send_id});
    if r.state == AskState::Answered {
        v["answer"] = serde_json::to_value(&r.answer).unwrap_or(Value::Null);
        v["via"] = serde_json::to_value(r.via).unwrap_or(Value::Null);
        v["transcript"] = json!(r.transcript);
        v["answered_at"] = serde_json::to_value(r.answered_at).unwrap_or(Value::Null);
    }
    v
}

fn human_send(v: &Value) -> String {
    let mut lines = vec![format!(
        "sent {} to position {} ({})",
        v["send_id"].as_str().unwrap_or_default(),
        v["slot"],
        v["status"].as_str().unwrap_or_default()
    )];
    if let Some(speech) = v.get("speech").filter(|s| !s.is_null()) {
        let model = speech["model"]
            .as_str()
            .map(|m| format!(", {m}"))
            .unwrap_or_default();
        lines.push(format!(
            "speech: {} ({} characters{model})",
            speech["status"].as_str().unwrap_or_default(),
            speech["chars"]
        ));
    }
    if let Some(ask) = v["ask_id"].as_str() {
        lines.push(format!("ask: {ask}"));
    }
    lines.join("\n")
}

fn human_wait(v: &Value) -> String {
    let id = v["ask_id"].as_str().unwrap_or_default();
    match v["state"].as_str().unwrap_or_default() {
        "answered" => {
            let a = &v["answer"];
            let answer = match a["kind"].as_str().unwrap_or_default() {
                "text" => a["text"].as_str().unwrap_or_default().to_owned(),
                "single_choice" => format!(
                    "{} [{}]",
                    a["label"].as_str().unwrap_or_default(),
                    a["option_id"].as_str().unwrap_or_default()
                ),
                "multiple_choice" => a["labels"]
                    .as_array()
                    .map(|l| {
                        l.iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default(),
                "slider" => a["value"].to_string(),
                "range" => format!("{} – {}", a["from"], a["to"]),
                _ => a.to_string(),
            };
            format!(
                "{id} answered by {}: {answer}",
                v["via"].as_str().unwrap_or("?")
            )
        }
        "pending" => {
            format!("{id}: no answer in time; it will arrive as a Ting event (peek.ask.answered)")
        }
        other => format!("{id}: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> SendArgs {
        SendArgs {
            speak: None,
            show: None,
            ask: None,
            voice: None,
            lang: None,
            duration: None,
            expires_in: None,
            notify: None,
            wait: None,
        }
    }

    fn code(a: &SendArgs) -> Option<ErrorCode> {
        prepare(a, &[]).err().map(|e| e.code().clone())
    }

    #[test]
    fn limits_and_flags() {
        let mut a = args();
        assert_eq!(code(&a), Some(ErrorCode::NothingToSend));
        a.speak = Some("x".repeat(2001));
        assert_eq!(code(&a), Some(ErrorCode::SpeakTooLong));
        a.speak = Some("é".repeat(2000));
        assert!(prepare(&a, &[]).is_ok());
        a.show = Some(r#"{"elements":[{"type":"text","text":"hi"}]}"#.into());
        a.ask = Some(r#"{"question":"q","type":"text"}"#.into());
        assert_eq!(code(&a), Some(ErrorCode::ConflictingFlags));
        a.ask = None;
        a.show = Some(format!(
            r#"{{"elements":[{{"type":"text","text":"{}"}}]}}"#,
            "t".repeat(161)
        ));
        assert_eq!(code(&a), Some(ErrorCode::TextTooLong));
        a.show = Some(r#"{"elements":[{"type":"text","text":"a"},{"type":"text","text":"b"},{"type":"text","text":"c"},{"type":"text","text":"d"}]}"#.into());
        assert_eq!(code(&a), Some(ErrorCode::TooManyElements));
        a.show = Some(r#"{"elements":[{"type":"text","text":"a","extra":1}]}"#.into());
        assert_eq!(code(&a), Some(ErrorCode::InvalidInput));
        a.show = Some(r#"{"elements":[{"type":"text","text":"a"}],"elements":[]}"#.into());
        assert_eq!(code(&a), Some(ErrorCode::InvalidJson));
        a.show = None;
        a.duration = Some(0);
        assert_eq!(code(&a), Some(ErrorCode::InvalidInput));
        a.duration = Some(121);
        assert_eq!(code(&a), Some(ErrorCode::InvalidInput));
        a.duration = Some(120);
        assert!(prepare(&a, &[]).is_ok());
        a.wait = Some(60);
        assert_eq!(code(&a), Some(ErrorCode::ConflictingFlags));
    }

    #[test]
    fn asks_and_waits() {
        let mut a = args();
        a.ask = Some(format!(
            r#"{{"question":"{}","type":"text"}}"#,
            "q".repeat(81)
        ));
        assert_eq!(code(&a), Some(ErrorCode::QuestionTooLong));
        a.ask = Some(
            r#"{"question":"q","type":"single_choice","options":["1","2","3","4","5","6","7"]}"#
                .into(),
        );
        assert_eq!(code(&a), Some(ErrorCode::TooManyOptions));
        a.ask =
            Some(r#"{"question":"q","type":"single_choice","options":["Keep","Delete"]}"#.into());
        a.wait = Some(601);
        assert_eq!(code(&a), Some(ErrorCode::InvalidInput));
        a.wait = Some(600);
        a.expires_in = Some(9);
        assert_eq!(code(&a), Some(ErrorCode::InvalidInput));
        a.expires_in = Some(10);
        let p = prepare(&a, &[Notify::SpeechFinished]);
        assert!(p.as_ref().is_ok_and(|p| p.op.wait
            && p.wait == Some(Duration::from_secs(600))
            && p.op.notify == vec![Notify::SpeechFinished]));
    }

    #[test]
    fn images_become_blobs() -> Result<()> {
        let t = tempfile::tempdir().map_err(|e| Error::internal(e.to_string()))?;
        let png = t.path().join("a.png");
        std::fs::write(&png, b"\x89PNG\r\n\x1a\n0000")
            .map_err(|e| Error::internal(e.to_string()))?;
        let mut a = args();
        a.show = Some(format!(
            r#"{{"elements":[{{"type":"image","path":"{p}","caption":"c"}},{{"type":"image","path":"{p}"}}]}}"#,
            p = png.display()
        ));
        let p = prepare(&a, &[])?;
        assert_eq!(p.blobs.len(), 2, "one blob per reference");
        let v = serde_json::to_value(&p.op).map_err(|e| Error::internal(e.to_string()))?;
        assert_eq!(v["show"]["elements"][0]["path"], json!({"blob":0}));
        assert_eq!(v["show"]["elements"][1]["path"], json!({"blob":1}));
        a.show =
            Some(r#"{"elements":[{"type":"image","path":"./definitely-missing.png"}]}"#.into());
        assert_eq!(code(&a), Some(ErrorCode::ImageUnreadable));
        Ok(())
    }
}
