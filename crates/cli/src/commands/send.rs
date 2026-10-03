//! `peek send` (BLUEPRINT §1.9.3, §1.9.6, §7.4; 0.1.2 contract §5).
//!
//! Everything is validated locally before peekd is contacted: flag
//! combinations, `--speak`, the `--show`/`--ask` JSON (strict: duplicate or
//! unknown keys refused), every limit, deadlines and schedules (`--in`,
//! `--at`, `--expires-in`, `--expires-at`, `--tz`), and every image, whose
//! bytes the CLI reads (relative to the current directory) and sends as
//! frame blobs so Peek.app never opens the Silicon's paths. With `--wait`
//! the connection stays open for the `ask.result` event; on timeout or
//! disconnect the answer falls back to Ting.
//!
//! New flags are refused with `app_update_pending` against a peekd that does
//! not announce their feature (an older peekd would silently ignore them).

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use silicon_peek_client::{
    Error, ErrorCode, Result,
    ipc::cli::{AskResult, AskState, SendOp, SendResult, Warning, features, warnings},
    runtime::daemon::{EventWait, REQUEST_TIMEOUT},
    schema::{
        ImageRef,
        ask::Ask,
        send::{
            Notify, SendFlags, check_duration, check_flags, check_isi, check_speak, check_voice,
            check_voice_instructions, check_wait, normalize_language, parse_expires_in,
            parse_schedule_in,
        },
        show::Show,
    },
    timestamp::Timestamp,
};

use super::{mac_session, next, require_features};
use crate::{
    cli::SendArgs,
    context::Globals,
    input,
    output::Out,
    service::{self, require_mac},
    when::{self, LocalZone, TzArg},
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
    /// Local notes merged into peekd's (e.g. `isi_ignored`).
    pub warnings: Vec<Warning>,
    /// `notify` came from the home's config, not `--notify`.
    pub notify_from_config: bool,
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

/// Validates the arguments and reads the images (no peekd, no network), at
/// the current time and in the Mac's time zone.
pub fn prepare(args: &SendArgs, default_notify: &[Notify]) -> Result<Prepared> {
    prepare_at(
        args,
        default_notify,
        Timestamp::now(),
        &when::mac_local_zone,
    )
}

/// [`prepare`] at `now`, with `local` giving the Mac's zone (read only when
/// an `--at`/`--expires-at` value has neither an offset nor `--tz`).
#[allow(clippy::too_many_lines)] // one validation pass, in the contract's order
pub fn prepare_at(
    args: &SendArgs,
    default_notify: &[Notify],
    now: Timestamp,
    local: &dyn Fn() -> LocalZone,
) -> Result<Prepared> {
    check_flags(SendFlags {
        speak: args.speak.is_some(),
        show: args.show.is_some(),
        ask: args.ask.is_some(),
        voice: args.voice.is_some(),
        voice_instructions: args.voice_instructions.is_some(),
        lang: args.lang.is_some(),
        duration: args.duration.is_some(),
        expires_in: args.expires_in.is_some(),
        wait: args.wait.is_some(),
        expires_at: args.expires_at.is_some(),
        schedule_in: args.in_.is_some(),
        schedule_at: args.at.is_some(),
        tz: args.tz.is_some(),
        replace: args.replace,
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
        check_voice(v).map_err(|e| e.with_input_field("--voice", ""))?;
    }
    if let Some(instructions) = &args.voice_instructions {
        check_voice_instructions(instructions)
            .map_err(|e| e.with_input_field("--voice-instructions", ""))?;
    }
    let lang = args
        .lang
        .as_deref()
        .map(|l| normalize_language(l).map_err(|e| e.with_input_field("--lang", "")))
        .transpose()?;
    let duration_ms = args
        .duration
        .map(|s| check_duration(s).map(|d| d.as_secs() * 1000))
        .transpose()?;
    let wait = args.wait.map(|s| check_wait(Some(s))).transpose()?;
    let notify_from_config = args.notify.is_none();
    let notify = match &args.notify {
        Some(list) => Notify::parse_list(list).map_err(|e| {
            e.with_input_field("--notify", "--notify speech_finished,show_dismissed,shown")
        })?,
        None => default_notify.to_vec(),
    };
    let mut warnings = Vec::new();
    // ISI is optional context: an invalid one is left out, never a reason
    // for the Carbon to miss the bubble.
    let isi = match std::env::var("ISI") {
        Ok(v) if !v.is_empty() => match check_isi(&v) {
            Ok(()) => Some(v),
            Err(e) => {
                warnings.push(Warning {
                    code: warnings::ISI_IGNORED.to_owned(),
                    message: format!(
                        "the ISI environment variable was left out of this send: {} (at most 160 characters, one line)",
                        e.message()
                    ),
                    details: Some(json!({"field": "ISI"})),
                });
                None
            }
        },
        _ => None,
    };
    // Deadlines and schedules. The Mac's zone is read only when a value
    // needs it.
    let tz_arg: Option<TzArg> = args.tz.as_deref().map(when::parse_tz).transpose()?;
    let needs_local = [args.at.as_deref(), args.expires_at.as_deref()]
        .into_iter()
        .flatten()
        .any(|raw| when::needs_local(raw, tz_arg.as_ref()));
    let local_zone = if needs_local {
        let z = local();
        if z.fallback_utc {
            warnings.push(Warning {
                code: warnings::TIMEZONE_FALLBACK_UTC.to_owned(),
                message: "the Mac's time zone could not be read (/etc/localtime), so --at/--expires-at without an offset were read as UTC".to_owned(),
                details: None,
            });
        }
        z
    } else {
        LocalZone {
            zone: jiff::tz::TimeZone::UTC,
            name: None,
            fallback_utc: false,
        }
    };
    let expires_in_s = args
        .expires_in
        .as_deref()
        .map(|raw| parse_expires_in(raw).map(|d| d.as_secs()))
        .transpose()?;
    let mut tz = None;
    let due_at = if let Some(raw) = &args.in_ {
        Some(now.plus(parse_schedule_in(raw)?))
    } else if let Some(raw) = &args.at {
        let r = when::resolve("--at", raw, tz_arg.as_ref(), &local_zone, now)?;
        tz = r.tz;
        Some(r.at)
    } else {
        None
    };
    let expires_at = args
        .expires_at
        .as_deref()
        .map(|raw| when::resolve("--expires-at", raw, tz_arg.as_ref(), &local_zone, now))
        .transpose()?
        .map(|r| {
            if tz.is_none() {
                tz = r.tz;
            }
            r.at
        });
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
        voice_instructions: args.voice_instructions.clone(),
        lang,
        notify,
        duration_ms,
        expires_in_s,
        wait: wait.is_some(),
        expires_at,
        due_at,
        tz,
        replace: args.replace,
    };
    // The same checks peekd runs (strict here: no slack for latency).
    op.validate_at(&blobs, now, 0)?;
    Ok(Prepared {
        op,
        blobs,
        wait,
        warnings,
        notify_from_config,
    })
}

/// The features a prepared send needs from peekd (contract §5.8). A `shown`
/// from the config default is not listed: it is dropped instead.
fn needed_features(p: &Prepared) -> Vec<(&'static str, &'static str)> {
    let mut needed = Vec::new();
    let op = &p.op;
    if op.speak.is_some() {
        needed.push((features::ELEVENLABS_TTS, "ElevenLabs v4 speech"));
    }
    if (op.expires_in_s.is_some() && op.ask.is_none()) || op.expires_at.is_some() {
        needed.push((
            features::EXPIRY_ALL,
            "--expires-in/--expires-at on --speak and --show",
        ));
    }
    if op.due_at.is_some() {
        needed.push((features::SCHEDULE, "scheduled sends (--in/--at)"));
    }
    if op.replace {
        needed.push((features::REPLACE, "--replace"));
    }
    if op.notify.contains(&Notify::Shown) && !p.notify_from_config {
        needed.push((features::NOTIFY_SHOWN, "--notify shown"));
    }
    needed
}

pub async fn run(g: &Globals, out: Out, args: SendArgs) -> Result<()> {
    require_mac("send")?;
    g.check_stdin(&[
        ("--show", args.show.as_deref() == Some("-")),
        ("--ask", args.ask.as_deref() == Some("-")),
    ])?;
    // Validate before touching the session so input errors are deterministic.
    let config = crate::context::existing_store()
        .ok()
        .flatten()
        .and_then(|s| s.read_config().ok())
        .unwrap_or_default();
    let mut prepared = prepare(&args, &config.notify)?;
    if prepared.op.speak.is_some() {
        prepared.op.voice = prepared.op.voice.or_else(|| config.voice.clone());
        prepared.op.lang = prepared.op.lang.or_else(|| config.language.clone());
        prepared.op.voice_instructions = prepared
            .op
            .voice_instructions
            .or_else(|| config.voice_instructions.clone());
    }
    let (_session, auth) = mac_session(g).await?;
    let mut svc = service::ensure_service().await?;
    require_features(&svc, &needed_features(&prepared))?;
    prepared
        .warnings
        .extend(super::register::apply_defaults(&mut svc, &auth, &config).await?);
    if prepared.notify_from_config
        && prepared.op.notify.contains(&Notify::Shown)
        && !svc.hello.has_feature(features::NOTIFY_SHOWN)
    {
        prepared.op.notify.retain(|n| *n != Notify::Shown);
        out.hint(format!(
            "note: config notify \"shown\" is not supported by peekd {} yet; this send goes without it",
            svc.hello.peekd_version
        ));
    }
    // Relative schedules start after setup (drawing validation can take 120 s).
    if let Some(raw) = &args.in_ {
        prepared.op.due_at = Some(Timestamp::now().plus(parse_schedule_in(raw)?));
    }
    let started = Instant::now();
    let (mut result, _) = svc
        .call(&prepared.op, Some(&auth), prepared.blobs, REQUEST_TIMEOUT)
        .await?;
    result.warnings.splice(0..0, prepared.warnings);
    out.warnings(&result.warnings);
    // The zone is read only when there is a time to show.
    let zone = if result.due_at.is_some() || result.expires_at.is_some() {
        display_zone(result.tz.as_deref())
    } else {
        LocalZone {
            zone: jiff::tz::TimeZone::UTC,
            name: None,
            fallback_utc: false,
        }
    };
    let now = Timestamp::now();
    let Some(wait) = prepared.wait else {
        out.result(&result, |v| human_send(v, &zone, now));
        next(
            out,
            &send_hints(&result)
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        );
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

/// The zone results are shown in: the send's `tz`, else the Mac's zone.
pub(crate) fn display_zone(tz: Option<&str>) -> LocalZone {
    tz.and_then(|t| when::parse_tz(t).ok())
        .map_or_else(when::mac_local_zone, |t| LocalZone {
            zone: t.zone,
            name: Some(t.name),
            fallback_utc: false,
        })
}

fn send_hints(result: &SendResult) -> Vec<String> {
    use silicon_peek_client::ipc::cli::SendStatus;
    let mut hints = Vec::new();
    match (result.status, &result.schedule_id) {
        (SendStatus::Scheduled, Some(sch)) => {
            hints.push("peek schedule list".to_owned());
            hints.push(format!("peek schedule cancel {sch}"));
        }
        (SendStatus::Queued, _) if result.queue_position.is_some() => {
            hints.push("peek queue".to_owned());
        }
        _ => {}
    }
    if let Some(ask) = &result.ask_id {
        hints.push(format!("peek ask get {ask}"));
        hints.push("answers arrive as Ting events peek.ask.answered (peek docs ting)".to_owned());
    } else if result.status == SendStatus::Showing {
        hints.push("peek history".to_owned());
    }
    hints
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

fn ts(v: &Value) -> Option<Timestamp> {
    v.as_str().and_then(|s| Timestamp::parse(s).ok())
}

/// The human `peek send` lines (contract §5.5).
pub(crate) fn human_send(v: &Value, zone: &LocalZone, now: Timestamp) -> String {
    let id = v["send_id"].as_str().unwrap_or_default();
    let slot = &v["slot"];
    let status = v["status"].as_str().unwrap_or_default();
    let has_warning = |code: &str| {
        v["warnings"]
            .as_array()
            .is_some_and(|w| w.iter().any(|w| w["code"] == code))
    };
    let first = match (status, v["queue_position"].as_u64()) {
        ("scheduled", _) => {
            let due = ts(&v["due_at"]).unwrap_or(now);
            format!(
                "scheduled {id} for {} ({}); schedule {}",
                when::render_local(due, &zone.zone, zone.name.as_deref()),
                when::render_relative(due, now),
                v["schedule_id"].as_str().unwrap_or_default()
            )
        }
        ("showing", _) => match v["replaced_send_id"].as_str() {
            Some(old) => format!("sent {id} to position {slot} (showing; replaced {old})"),
            None => format!("sent {id} to position {slot} (showing)"),
        },
        ("queued", Some(0)) => {
            let why = if has_warning("carbon_away") {
                "shown when the Carbon is back"
            } else if has_warning("carbon_paused") {
                "Peek is paused; shown when the Carbon resumes"
            } else {
                "shown when Peek.app starts"
            };
            format!("sent {id} to position {slot} (queued: {why})")
        }
        ("queued", Some(ahead)) => {
            format!("sent {id} to position {slot} (queued: {ahead} ahead of it)")
        }
        // An older peekd reports no queue position.
        (other, _) => format!("sent {id} to position {slot} ({other})"),
    };
    let mut lines = vec![first];
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
    if let Some(exp) = ts(&v["expires_at"]) {
        lines.push(format!(
            "expires: {} ({})",
            when::render_local(exp, &zone.zone, zone.name.as_deref()),
            when::render_relative(exp, now)
        ));
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
        "replaced" => format!("{id}: replaced by a newer send from you (no answer)"),
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
            voice_instructions: None,
            lang: None,
            duration: None,
            expires_in: None,
            expires_at: None,
            in_: None,
            at: None,
            tz: None,
            replace: false,
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

    fn field(a: &SendArgs) -> Option<String> {
        prepare(a, &[]).err().and_then(|e| {
            assert!(e.hint().is_some(), "every input error has a hint: {e}");
            e.details()
                .and_then(|d| d.get("field"))
                .and_then(|f| f.as_str().map(str::to_owned))
        })
    }

    #[test]
    fn input_errors_name_the_field() {
        let mut a = args();
        a.ask = Some(
            r#"{"question":"Pick","type":"single_choice","options":["Same"," same "]}"#.into(),
        );
        assert_eq!(code(&a), Some(ErrorCode::InvalidInput), "duplicate labels");
        assert_eq!(field(&a).as_deref(), Some("ask.options[1].label"));
        a.ask = Some(r#"{"question":"Pick","type":"single_choice","options":["a"]}"#.into());
        assert_eq!(field(&a).as_deref(), Some("ask.options"));
        a.ask = Some(
            r#"{"question":"Pick","type":"single_choice","options":[{"id":"x","label":"a"},{"id":"x","label":"b"}]}"#
                .into(),
        );
        assert_eq!(field(&a).as_deref(), Some("ask.options[1].id"));
        a.ask = Some(r#"{"question":"q","type":"slider","min":0,"max":10,"step":0}"#.into());
        assert_eq!(field(&a).as_deref(), Some("ask.step"));
        a.ask = Some(r#"{"question":"q","type":"text","bogus":1}"#.into());
        assert_eq!(field(&a).as_deref(), Some("ask.bogus"));
        a.ask = Some(r#"{"question":"q","type":"text","max_length":2001}"#.into());
        assert_eq!(field(&a).as_deref(), Some("ask.max_length"));
        a.ask = Some("{not json".into());
        assert_eq!(code(&a), Some(ErrorCode::InvalidJson));
        a.ask = None;
        a.show = Some(r#"{"elements":[]}"#.into());
        assert_eq!(field(&a).as_deref(), Some("show.elements"));
        a.show = None;
        a.speak = Some("hola".into());
        a.lang = Some("es-MX".into());
        let p = prepare(&a, &[]);
        assert_eq!(
            p.ok().and_then(|p| p.op.lang).as_deref(),
            Some("es"),
            "a full BCP 47 tag uses its primary subtag"
        );
        a.lang = Some("not a tag".into());
        assert_eq!(field(&a).as_deref(), Some("--lang"));
        a.lang = None;
        a.voice = Some("not a voice".into());
        assert_eq!(field(&a).as_deref(), Some("--voice"));
        a.voice = None;
        a.notify = Some("nope".into());
        assert_eq!(field(&a).as_deref(), Some("--notify"));
    }

    #[test]
    fn elevenlabs_voice_instructions_reach_the_daemon_unchanged() -> Result<()> {
        let mut a = args();
        a.speak = Some("[Indian accent] Anuv Jain".into());
        a.voice = Some("JBFqnCBsd6RMkjVDRZzb".into());
        a.voice_instructions = Some("Warm and conversational.\nSlow down for names.".into());
        a.lang = Some("hi-IN".into());
        let p = prepare(&a, &[])?;
        assert_eq!(p.op.speak, a.speak);
        assert_eq!(p.op.voice_instructions, a.voice_instructions);
        assert_eq!(p.op.lang.as_deref(), Some("hi"));
        assert!(
            needed_features(&p)
                .iter()
                .any(|(f, _)| *f == features::ELEVENLABS_TTS)
        );
        a.voice_instructions = Some("x".repeat(2001));
        assert_eq!(field(&a).as_deref(), Some("--voice-instructions"));
        a.voice_instructions = Some("Warm".into());
        a.speak = None;
        a.show = Some(r#"{"elements":[{"type":"text","text":"hi"}]}"#.into());
        a.voice = None;
        a.lang = None;
        assert_eq!(code(&a), Some(ErrorCode::ConflictingFlags));
        Ok(())
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
        a.expires_in = Some("9".into());
        assert_eq!(code(&a), Some(ErrorCode::InvalidInput));
        a.expires_in = Some("10".into());
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
        a.show = Some(
            json!({"elements": [
                {"type":"image","path":png,"caption":"c"},
                {"type":"image","path":png}
            ]})
            .to_string(),
        );
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

    fn now() -> Timestamp {
        Timestamp::parse("2026-09-27T06:12:00Z").unwrap_or(Timestamp::from_unix(0))
    }

    fn kolkata() -> LocalZone {
        LocalZone {
            zone: jiff::tz::db()
                .get("Asia/Kolkata")
                .unwrap_or(jiff::tz::TimeZone::UTC),
            name: Some("Asia/Kolkata".to_owned()),
            fallback_utc: false,
        }
    }

    fn at_now(a: &SendArgs) -> Result<Prepared> {
        prepare_at(a, &[], now(), &kolkata)
    }

    fn speak_args() -> SendArgs {
        let mut a = args();
        a.speak = Some("hi".into());
        a
    }

    #[test]
    fn deadlines_and_schedules() -> Result<()> {
        let mut a = speak_args();
        a.expires_in = Some("15m".into());
        assert_eq!(at_now(&a)?.op.expires_in_s, Some(900), "any kind now");
        let mut a = args();
        a.ask = Some(r#"{"question":"q","type":"text"}"#.into());
        a.expires_in = Some("60".into());
        assert_eq!(
            at_now(&a)?.op.expires_in_s,
            Some(60),
            "0.1.1's plain seconds"
        );
        let mut a = speak_args();
        a.expires_at = Some("18:00".into());
        let p = at_now(&a)?;
        assert_eq!(
            p.op.expires_at,
            Timestamp::parse("2026-09-27T12:30:00Z").ok()
        );
        assert_eq!(p.op.tz.as_deref(), Some("Asia/Kolkata"));
        let mut a = speak_args();
        a.in_ = Some("2h".into());
        let p = at_now(&a)?;
        assert_eq!(p.op.due_at, Some(now().plus(Duration::from_secs(7200))));
        assert_eq!(p.op.tz, None);
        let mut a = speak_args();
        a.at = Some("2026-09-27T18:00+05:30".into());
        a.expires_at = Some("2026-09-27T18:30".into());
        a.tz = Some("Europe/Berlin".into());
        let p = at_now(&a)?;
        assert_eq!(p.op.due_at, Timestamp::parse("2026-09-27T12:30:00Z").ok());
        assert_eq!(
            p.op.expires_at,
            Timestamp::parse("2026-09-27T16:30:00Z").ok()
        );
        assert_eq!(
            p.op.tz.as_deref(),
            Some("Europe/Berlin"),
            "the --expires-at zone is echoed"
        );
        a.replace = true;
        assert!(at_now(&a)?.op.replace);
        Ok(())
    }

    #[test]
    fn deadline_errors_name_their_field() {
        let err = |a: &SendArgs| at_now(a).err();
        let field = |a: &SendArgs| {
            err(a).and_then(|e| {
                assert!(e.hint().is_some(), "every input error has a hint: {e}");
                e.details()
                    .and_then(|d| d.get("field"))
                    .and_then(|f| f.as_str().map(str::to_owned))
            })
        };
        let mut a = speak_args();
        a.at = Some("2026-09-27T18:00".into());
        a.expires_in = Some("10m".into());
        assert_eq!(
            err(&a).map(|e| e.code().clone()),
            Some(ErrorCode::ConflictingFlags)
        );
        let mut a = speak_args();
        a.tz = Some("Asia/Kolkata".into());
        assert_eq!(
            err(&a).map(|e| e.code().clone()),
            Some(ErrorCode::ConflictingFlags)
        );
        let mut a = speak_args();
        a.at = Some("09:00".into());
        assert_eq!(field(&a).as_deref(), Some("--at"));
        let mut a = speak_args();
        a.at = Some("soon".into());
        assert_eq!(field(&a).as_deref(), Some("--at"));
        let mut a = speak_args();
        a.in_ = Some("366d".into());
        assert_eq!(field(&a).as_deref(), Some("--in"));
        let mut a = speak_args();
        a.expires_in = Some("5s".into());
        assert_eq!(field(&a).as_deref(), Some("--expires-in"));
        let mut a = speak_args();
        a.expires_at = Some("2026-09-27T11:42:05".into());
        assert_eq!(
            field(&a).as_deref(),
            Some("--expires-at"),
            "less than 10 s ahead"
        );
        let mut a = speak_args();
        a.tz = Some("Mars/Olympus".into());
        a.at = Some("18:00".into());
        assert_eq!(field(&a).as_deref(), Some("--tz"));
        let mut a = speak_args();
        a.in_ = Some("1h".into());
        a.expires_at = Some("2026-09-27T12:00".into());
        assert_eq!(
            field(&a).as_deref(),
            Some("--expires-at"),
            "must be after the due time"
        );
        let mut a = args();
        a.ask = Some(r#"{"question":"q","type":"text"}"#.into());
        a.in_ = Some("1h".into());
        a.wait = Some(60);
        assert_eq!(
            err(&a).map(|e| e.code().clone()),
            Some(ErrorCode::ConflictingFlags)
        );
        let mut a = speak_args();
        a.notify = Some("shown,nope".into());
        assert_eq!(field(&a).as_deref(), Some("--notify"));
    }

    #[test]
    fn the_mac_zone_is_read_only_when_needed() -> Result<()> {
        let never = || -> LocalZone { panic!("the Mac's zone was read") };
        let mut a = speak_args();
        a.at = Some("2026-09-27T18:00Z".into());
        prepare_at(&a, &[], now(), &never)?;
        a.at = Some("18:00".into());
        a.tz = Some("Asia/Kolkata".into());
        prepare_at(&a, &[], now(), &never)?;
        let mut a = speak_args();
        a.in_ = Some("5m".into());
        prepare_at(&a, &[], now(), &never)?;
        // An unreadable zone falls back to UTC with a warning.
        let utc = || LocalZone {
            zone: jiff::tz::TimeZone::UTC,
            name: None,
            fallback_utc: true,
        };
        let mut a = speak_args();
        a.at = Some("18:00".into());
        let p = prepare_at(&a, &[], now(), &utc)?;
        assert!(p.warnings.iter().any(|w| w.code == "timezone_fallback_utc"));
        assert_eq!(p.op.due_at, Timestamp::parse("2026-09-27T18:00:00Z").ok());
        Ok(())
    }

    #[test]
    fn features_follow_the_flags() -> Result<()> {
        let names = |p: &Prepared| {
            needed_features(p)
                .into_iter()
                .map(|(f, _)| f)
                .collect::<Vec<_>>()
        };
        let p = at_now(&speak_args())?;
        assert_eq!(names(&p), vec![features::ELEVENLABS_TTS]);
        let mut a = args();
        a.ask = Some(r#"{"question":"q","type":"text"}"#.into());
        a.expires_in = Some("60".into());
        assert!(
            names(&at_now(&a)?).is_empty(),
            "--expires-in on an ask worked in 0.1.1"
        );
        let mut a = speak_args();
        a.expires_in = Some("60".into());
        a.in_ = None;
        assert_eq!(
            names(&at_now(&a)?),
            vec![features::ELEVENLABS_TTS, features::EXPIRY_ALL]
        );
        let mut a = speak_args();
        a.in_ = Some("1h".into());
        a.replace = true;
        a.notify = Some("shown".into());
        assert_eq!(
            names(&at_now(&a)?),
            vec![
                features::ELEVENLABS_TTS,
                features::SCHEDULE,
                features::REPLACE,
                features::NOTIFY_SHOWN
            ]
        );
        let p = prepare_at(&speak_args(), &[Notify::Shown], now(), &kolkata)?;
        assert!(p.notify_from_config);
        assert_eq!(
            names(&p),
            vec![features::ELEVENLABS_TTS],
            "a config default is dropped, not refused"
        );
        Ok(())
    }

    #[test]
    fn human_send_lines() {
        let z = kolkata();
        let base = |status: &str, extra: Value| {
            let mut v = json!({"send_id": "snd_1", "ask_id": null, "slot": 3, "status": status,
                "speech": null, "warnings": [], "queue_position": 0, "waiting": 0, "expires_at": null,
                "schedule_id": null, "due_at": null, "tz": null, "replaced_send_id": null});
            if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
                o.extend(e.clone());
            }
            human_send(&v, &z, now())
        };
        assert_eq!(
            base("showing", json!({})),
            "sent snd_1 to position 3 (showing)"
        );
        assert_eq!(
            base("showing", json!({"replaced_send_id": "snd_0"})),
            "sent snd_1 to position 3 (showing; replaced snd_0)"
        );
        assert_eq!(
            base(
                "queued",
                json!({"warnings": [{"code": "carbon_away", "message": "m"}]})
            ),
            "sent snd_1 to position 3 (queued: shown when the Carbon is back)"
        );
        assert_eq!(
            base(
                "queued",
                json!({"warnings": [{"code": "carbon_paused", "message": "m"}]})
            ),
            "sent snd_1 to position 3 (queued: Peek is paused; shown when the Carbon resumes)"
        );
        assert_eq!(
            base("queued", json!({})),
            "sent snd_1 to position 3 (queued: shown when Peek.app starts)"
        );
        assert_eq!(
            base("queued", json!({"queue_position": 2, "waiting": 2})),
            "sent snd_1 to position 3 (queued: 2 ahead of it)"
        );
        assert_eq!(
            base(
                "scheduled",
                json!({"queue_position": null, "waiting": null,
                "due_at": "2026-09-27T12:30:00Z", "schedule_id": "sch_1"})
            ),
            "scheduled snd_1 for 2026-09-27 18:00 IST (Asia/Kolkata) (in 6h 18m); schedule sch_1"
        );
        assert_eq!(
            base(
                "showing",
                json!({"ask_id": "ask_1", "expires_at": "2026-09-27T06:22:00Z",
                "speech": {"status": "pending", "model": "JBFqnCBsd6RMkjVDRZzb", "chars": 5}})
            ),
            "sent snd_1 to position 3 (showing)\nspeech: pending (5 characters, JBFqnCBsd6RMkjVDRZzb)\nask: ask_1\nexpires: 2026-09-27 11:52 IST (Asia/Kolkata) (in 10m)"
        );
        // An older peekd has no queue position.
        let old = json!({"send_id": "snd_1", "ask_id": null, "slot": 3, "status": "queued", "warnings": []});
        assert_eq!(
            human_send(&old, &z, now()),
            "sent snd_1 to position 3 (queued)"
        );
        assert_eq!(
            human_wait(&json!({"ask_id": "ask_1", "state": "replaced"})),
            "ask_1: replaced by a newer send from you (no answer)"
        );
    }
}
