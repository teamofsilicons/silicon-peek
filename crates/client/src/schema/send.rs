//! Validators for `peek send` options (BLUEPRINT §7.4) and the TTS voice and
//! language settings shared with `peek config` (§7.3).

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{Lines, check_text, limits};
use crate::error::{Error, ErrorCode, Result};

/// George, Peek's default multilingual `ElevenLabs` voice.
pub const DEFAULT_TTS_VOICE: &str = "JBFqnCBsd6RMkjVDRZzb";

/// Opt-in notifications (`--notify`, config `notify`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Notify {
    /// Send `peek.speech.finished` when a `--speak` finishes or is stopped.
    SpeechFinished,
    /// Send `peek.show.dismissed` when the Carbon closes a `--show` early.
    ShowDismissed,
    /// Send `peek.send.shown` when it appears (scheduled sends always do).
    Shown,
}

impl Notify {
    /// Every value.
    pub const ALL: [Notify; 3] = [Notify::SpeechFinished, Notify::ShowDismissed, Notify::Shown];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SpeechFinished => "speech_finished",
            Self::ShowDismissed => "show_dismissed",
            Self::Shown => "shown",
        }
    }

    /// Parses one value.
    ///
    /// # Errors
    /// `invalid_input` naming the allowed values.
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "speech_finished" => Ok(Self::SpeechFinished),
            "show_dismissed" => Ok(Self::ShowDismissed),
            "shown" => Ok(Self::Shown),
            other => Err(Error::invalid_input(format!(
                "`{other}` is not a notification; allowed: speech_finished, show_dismissed, shown"
            ))
            .with_hint("use speech_finished, show_dismissed and/or shown")
            .with_details(json!({"allowed": ["speech_finished", "show_dismissed", "shown"]}))),
        }
    }

    /// Parses a comma-separated `--notify` list, sorted and deduplicated.
    ///
    /// # Errors
    /// `invalid_input` for an unknown entry.
    pub fn parse_list(s: &str) -> Result<Vec<Self>> {
        let mut out = Vec::new();
        for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            out.push(Self::parse(part)?);
        }
        out.sort_unstable();
        out.dedup();
        Ok(out)
    }
}

/// Validates `--speak` text: 1–2000 characters; line breaks allowed.
///
/// # Errors
/// `speak_too_long` or `invalid_input`.
pub fn check_speak(text: &str) -> Result<()> {
    check_text(
        "--speak",
        text,
        1,
        limits::SPEAK_MAX_CHARS,
        ErrorCode::SpeakTooLong,
        Lines::Multi,
    )
    .map_err(|e| {
        if *e.code() == ErrorCode::SpeakTooLong {
            e.with_hint("peek accepts at most 2000 characters per speech request; split it over several sends")
        } else {
            e
        }
    })?;
    check_speech_markup(text)
}

/// Validates an ISI (from `$ISI`): 1–160 characters, no control characters.
///
/// # Errors
/// `invalid_input`.
pub fn check_isi(isi: &str) -> Result<()> {
    check_text(
        "ISI",
        isi,
        1,
        limits::ISI_MAX_CHARS,
        ErrorCode::InvalidInput,
        Lines::Single,
    )
}

/// Validates an `ElevenLabs` voice ID (1–128 ASCII letters, digits,
/// underscores or hyphens). The provider checks voice availability.
///
/// # Errors
/// `invalid_input` with an example.
pub fn check_voice(voice: &str) -> Result<()> {
    if (1..=128).contains(&voice.len())
        && voice
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        Ok(())
    } else {
        Err(Error::invalid_input("voice must be an ElevenLabs voice ID of 1–128 ASCII letters, digits, underscores or hyphens")
            .with_hint("use George: JBFqnCBsd6RMkjVDRZzb, or another ElevenLabs voice ID"))
    }
}

/// Validates delivery instructions without changing text or inline markup.
///
/// # Errors
/// `invalid_input` for empty text, control characters or more than 2000 characters.
pub fn check_voice_instructions(instructions: &str) -> Result<()> {
    check_text(
        "--voice-instructions",
        instructions,
        1,
        limits::SPEAK_MAX_CHARS,
        ErrorCode::InvalidInput,
        Lines::Multi,
    )?;
    check_speech_markup(instructions)
}

fn check_speech_markup(text: &str) -> Result<()> {
    let has_tag = text.split('<').skip(1).any(|tail| {
        tail.split_once('>').is_some_and(|(tag, _)| {
            tag.trim_start_matches('/')
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphabetic)
        })
    });
    if has_tag {
        Err(Error::invalid_input("ElevenLabs speech does not support angle-bracket or scoped tags")
            .with_hint("use native square-bracket cues such as [Indian accent] Anuv Jain, or --voice-instructions; cues do not have closing tags"))
    } else {
        Ok(())
    }
}

/// Validates a BCP 47 language tag and returns its primary subtag,
/// lowercased (`EN` → `en`, `es-MX` → `es`, `zh-Hant-TW` → `zh`).
/// `ElevenLabs` voices are multilingual; use voice instructions for an accent.
///
/// # Errors
/// `invalid_input` unless the primary subtag is 2–3 ASCII letters and every
/// further subtag is 1–8 letters or digits.
pub fn normalize_language(lang: &str) -> Result<String> {
    let mut parts = lang.split(['-', '_']);
    let primary = parts.next().unwrap_or_default();
    let primary_ok =
        (2..=3).contains(&primary.len()) && primary.bytes().all(|b| b.is_ascii_alphabetic());
    let rest_ok =
        parts.all(|p| (1..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()));
    if primary_ok && rest_ok {
        Ok(primary.to_ascii_lowercase())
    } else {
        Err(Error::invalid_input(format!(
            "language `{lang}` is not a BCP 47 language tag; use a tag such as `en`, `es-MX` or `ja`"
        ))
        .with_hint("peek uses the primary subtag (the part before the first -) as a language hint; use voice instructions for an accent"))
    }
}

fn bounded_secs(flag: &str, secs: u64, min: u64, max: u64) -> Result<Duration> {
    if (min..=max).contains(&secs) {
        Ok(Duration::from_secs(secs))
    } else {
        Err(Error::invalid_input(format!(
            "{flag} {secs} is out of range; it must be {min}–{max} seconds"
        ))
        .with_hint(format!("pass {flag} with a value from {min} to {max}"))
        .with_details(json!({"field": flag, "flag": flag, "min": min, "max": max, "actual": secs})))
    }
}

/// `--duration`: 1–120 s.
///
/// # Errors
/// `invalid_input` outside the range.
pub fn check_duration(secs: u64) -> Result<Duration> {
    bounded_secs(
        "--duration",
        secs,
        limits::DURATION_MIN_S,
        limits::DURATION_MAX_S,
    )
}

/// `--expires-in`: 10 s – 7 days.
///
/// # Errors
/// `invalid_input` outside the range.
pub fn check_expires_in(secs: u64) -> Result<Duration> {
    bounded_secs(
        "--expires-in",
        secs,
        limits::EXPIRES_IN_MIN_S,
        limits::EXPIRES_IN_MAX_S,
    )
}

/// `--wait[=SECS]`: 1–600 s; `None` (bare `--wait`) means 120 s.
///
/// # Errors
/// `invalid_input` outside the range.
pub fn check_wait(secs: Option<u64>) -> Result<Duration> {
    bounded_secs(
        "--wait",
        secs.unwrap_or(limits::WAIT_DEFAULT_S),
        limits::WAIT_MIN_S,
        limits::WAIT_MAX_S,
    )
}

/// What `parse_duration` accepts, for messages.
const DURATION_EXPECTED: &str = "<int> or e.g. 90s, 15m, 2h, 1d, 1h30m";

/// Duration units, largest first, with their seconds.
const DURATION_UNITS: [(u8, u64); 4] = [(b'd', 86_400), (b'h', 3_600), (b'm', 60), (b's', 1)];

/// Parses a duration: a bare integer (seconds, 0.1.1-compatible:
/// `--expires-in 60`) or `[<n>d][<n>h][<n>m][<n>s]` (at least one part, each
/// unit at most once, in this order, lowercase, no spaces, each `<n>` 1–7
/// digits). Leading and trailing ASCII whitespace is trimmed. The value is
/// not range-checked here.
///
/// # Errors
/// `invalid_input` with details `{"field": flag, "value": raw, "expected": …}`.
pub fn parse_duration(flag: &str, raw: &str) -> Result<Duration> {
    let bad = || {
        Error::invalid_input(format!(
            "{flag} `{raw}` is not a duration; use whole seconds or units like 90s, 15m, 2h, 1d, 1h30m"
        ))
        .with_hint(format!("{flag} 15m   (units d, h, m, s, largest first; or plain seconds)"))
        .with_details(json!({"field": flag, "value": raw, "expected": DURATION_EXPECTED}))
    };
    let text = raw.trim_matches(|c: char| c.is_ascii_whitespace());
    if text.is_empty() {
        return Err(bad());
    }
    if text.bytes().all(|b| b.is_ascii_digit()) {
        // Plain seconds, as 0.1.1 took them (any size; the caller checks the range).
        return text
            .parse::<u64>()
            .map(Duration::from_secs)
            .map_err(|_| bad());
    }
    let bytes = text.as_bytes();
    let mut secs: u64 = 0;
    let mut next_unit = 0; // index into UNITS: units must come in this order, once each
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        let digits = &text[start..i];
        if digits.is_empty() || digits.len() > 7 || i >= bytes.len() {
            return Err(bad());
        }
        let unit = bytes[i];
        i += 1;
        let Some(pos) = DURATION_UNITS[next_unit..]
            .iter()
            .position(|(u, _)| *u == unit)
        else {
            return Err(bad());
        };
        let (_, factor) = DURATION_UNITS[next_unit + pos];
        next_unit += pos + 1;
        let n: u64 = digits.parse().map_err(|_| bad())?;
        secs = secs
            .checked_add(n.checked_mul(factor).ok_or_else(bad)?)
            .ok_or_else(bad)?;
    }
    Ok(Duration::from_secs(secs))
}

/// `--expires-in`: [`parse_duration`], then 10 s – 7 days.
///
/// # Errors
/// `invalid_input`.
pub fn parse_expires_in(raw: &str) -> Result<Duration> {
    let d = parse_duration("--expires-in", raw)?;
    check_expires_in(d.as_secs())
        .map_err(|e| e.with_hint("--expires-in takes 10s to 7d, e.g. 90s, 15m, 2h, 1d"))
}

/// `--in`: [`parse_duration`], then 1 s – 365 days.
///
/// # Errors
/// `invalid_input`.
pub fn parse_schedule_in(raw: &str) -> Result<Duration> {
    let d = parse_duration("--in", raw)?;
    let secs = d.as_secs();
    if (limits::SCHEDULE_IN_MIN_S..=limits::SCHEDULE_IN_MAX_S).contains(&secs) {
        Ok(d)
    } else {
        Err(Error::invalid_input(format!(
            "--in {} is out of range; it must be 1 s – 365 d",
            raw.trim()
        ))
        .with_hint("--in takes 1s to 365d, e.g. 45s, 10m, 2h, 3d")
        .with_details(json!({
            "field": "--in",
            "value": raw,
            "min": limits::SCHEDULE_IN_MIN_S,
            "max": limits::SCHEDULE_IN_MAX_S,
            "actual": secs
        })))
    }
}

/// Which content flags a send carries, for the cross-flag rules.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // one flag per CLI option, by design
pub struct SendFlags {
    /// `--speak` given.
    pub speak: bool,
    /// `--show` given.
    pub show: bool,
    /// `--ask` given.
    pub ask: bool,
    /// `--voice` given.
    pub voice: bool,
    /// `--voice-instructions` given.
    pub voice_instructions: bool,
    /// `--lang` given.
    pub lang: bool,
    /// `--duration` given.
    pub duration: bool,
    /// `--expires-in` given.
    pub expires_in: bool,
    /// `--wait` given.
    pub wait: bool,
    /// `--expires-at` given.
    pub expires_at: bool,
    /// `--in` given (the CLI knows; an op only has `due_at`).
    pub schedule_in: bool,
    /// `--at` given (for an op: `due_at` is set).
    pub schedule_at: bool,
    /// `--tz` given.
    pub tz: bool,
    /// `--replace` given (combines with everything).
    pub replace: bool,
}

impl SendFlags {
    /// Whether the send is scheduled (`--in` or `--at`).
    #[must_use]
    pub const fn due(&self) -> bool {
        self.schedule_in || self.schedule_at
    }
}

/// Enforces the flag combination rules (contract §5.2): at least one of
/// speak/show/ask; show and ask are exclusive; voice/instructions/lang need speak;
/// duration needs show or speak and never goes with ask; wait needs ask and
/// no schedule; one deadline; one schedule; a scheduled send takes only
/// `--expires-at`; `--tz` needs `--at` or `--expires-at`.
///
/// # Errors
/// `nothing_to_send` or `conflicting_flags`.
pub fn check_flags(f: SendFlags) -> Result<()> {
    if !(f.speak || f.show || f.ask) {
        return Err(Error::new(
            ErrorCode::NothingToSend,
            "peek send needs at least one of --speak, --show or --ask",
        )
        .with_hint(r#"peek send --speak "Build finished" --show '{"elements":[{"type":"text","text":"✓ build"}]}'"#));
    }
    let conflict = |msg: &str, hint: &str| {
        Err(Error::new(ErrorCode::ConflictingFlags, msg.to_owned()).with_hint(hint.to_owned()))
    };
    if f.show && f.ask {
        return conflict(
            "--show and --ask cannot be combined: a bubble either shows content or asks one question",
            "send the show first, then the ask (it queues behind), or put the context into the question",
        );
    }
    if (f.voice || f.voice_instructions || f.lang) && !f.speak {
        return conflict(
            "--voice, --voice-instructions and --lang apply only to --speak",
            "add --speak \"…\" or drop them",
        );
    }
    if f.duration && !f.show && !f.speak {
        return conflict(
            "--duration applies only to --show or --speak",
            "drop --duration",
        );
    }
    if f.duration && f.ask {
        return conflict(
            "--duration does not apply to --ask: an ask stays until it is answered, dismissed or expires",
            "use --expires-in <DURATION> or --expires-at <DATETIME> to bound an ask",
        );
    }
    if f.wait && !f.ask {
        return conflict("--wait applies only to --ask", "drop --wait, or add --ask");
    }
    if f.expires_in && f.expires_at {
        return conflict(
            "--expires-in and --expires-at cannot be combined; give one deadline",
            "--expires-in 15m, or --expires-at 2026-09-27T18:00",
        );
    }
    if f.schedule_in && f.schedule_at {
        return conflict(
            "--in and --at cannot be combined; a send is scheduled once",
            "--in 2h, or --at 2026-09-27T18:00",
        );
    }
    if f.due() && f.expires_in {
        return conflict(
            "a scheduled send (--in/--at) takes --expires-at, not --expires-in: its clock would start now, not at the due time",
            "--expires-at <DATETIME> (after the due time)",
        );
    }
    if f.due() && f.wait {
        return conflict(
            "--wait cannot be used with --in or --at: nobody waits for a scheduled ask",
            "drop --wait; the answer arrives as peek.ask.answered",
        );
    }
    if f.tz && !f.schedule_at && !f.expires_at {
        return conflict(
            "--tz applies only to --at and --expires-at",
            "drop --tz, or add --at <DATETIME>",
        );
    }
    Ok(())
}

/// Checks a `--tz` name as it travels to peekd: 1–64 characters of
/// `[A-Za-z0-9_+-/]` (the CLI resolved it against the tz database already).
///
/// # Errors
/// `invalid_input` with field `--tz`.
pub fn check_tz_name(tz: &str) -> Result<()> {
    let n = tz.chars().count();
    let ok = (1..=limits::TZ_NAME_MAX_CHARS).contains(&n)
        && tz
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'+' | b'-' | b'/'));
    if ok {
        Ok(())
    } else {
        Err(Error::invalid_input(format!(
            "`--tz {}` is not an IANA time zone name (1–{} characters of letters, digits, _ + - /)",
            crate::identity::truncate_for_message(tz),
            limits::TZ_NAME_MAX_CHARS
        ))
        .with_hint("e.g. Asia/Kolkata, Europe/Berlin, America/New_York, UTC")
        .with_details(json!({"field": "--tz", "value": tz})))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speak_boundaries() {
        assert!(check_speak("a").is_ok());
        assert!(check_speak(&"é".repeat(2000)).is_ok());
        let e = check_speak(&"é".repeat(2001)).err();
        assert!(e.is_some_and(|e| *e.code() == ErrorCode::SpeakTooLong));
        assert!(check_speak("").is_err());
        assert!(check_speak("line one\nline two").is_ok());
        assert!(check_speak("[Indian accent] Anuv Jain [laughs]").is_ok());
        assert!(check_speak("1 < 2 and 3 > 2").is_ok());
        for markup in [
            "<indian accent>Anuv Jain</indian accent>",
            "<whisper>hello</whisper>",
        ] {
            assert!(check_speak(markup).is_err());
            assert!(check_voice_instructions(markup).is_err());
        }
    }

    #[test]
    fn isi_boundaries() {
        assert!(check_isi("deliberate").is_ok());
        assert!(check_isi("name:session.id-1").is_ok());
        assert!(check_isi(&"i".repeat(160)).is_ok());
        assert!(check_isi(&"i".repeat(161)).is_err());
        assert!(check_isi("").is_err());
        assert!(check_isi("a\nb").is_err());
    }

    #[test]
    fn voices_and_languages() {
        assert!(check_voice("JBFqnCBsd6RMkjVDRZzb").is_ok());
        assert!(check_voice("custom_voice-123").is_ok());
        assert!(check_voice("DtsPFCrhbCbbJkwZsb3d").is_ok());
        assert!(check_voice("").is_err());
        assert!(check_voice("not a voice").is_err());
        assert!(check_voice("voice?key=secret").is_err());
        assert!(check_voice(&"x".repeat(129)).is_err());
        assert!(check_voice_instructions("Warm, calm.\n[Indian accent] Anuv Jain").is_ok());
        assert!(check_voice_instructions(&"é".repeat(2000)).is_ok());
        assert!(check_voice_instructions(&"é".repeat(2001)).is_err());
        assert!(check_voice_instructions("").is_err());
        assert!(check_voice_instructions("calm\0voice").is_err());
        assert_eq!(normalize_language("EN").ok().as_deref(), Some("en"));
        assert!(normalize_language("e").is_err());
        assert!(normalize_language("english").is_err());
        assert_eq!(normalize_language("es-MX").ok().as_deref(), Some("es"));
        assert_eq!(normalize_language("zh-Hant-TW").ok().as_deref(), Some("zh"));
        assert_eq!(normalize_language("pt_BR").ok().as_deref(), Some("pt"));
        assert!(normalize_language("en-").is_err());
        assert!(normalize_language("en-toolongsubtag").is_err());
        assert!(normalize_language("-US").is_err());
    }

    #[test]
    fn duration_boundaries() {
        assert!(check_duration(0).is_err());
        assert!(check_duration(1).is_ok());
        assert!(check_duration(120).is_ok());
        assert!(check_duration(121).is_err());
        assert!(check_expires_in(9).is_err());
        assert!(check_expires_in(10).is_ok());
        assert!(check_expires_in(604_800).is_ok());
        assert!(check_expires_in(604_801).is_err());
        assert_eq!(check_wait(None).ok(), Some(Duration::from_secs(120)));
        assert!(check_wait(Some(0)).is_err());
        assert!(check_wait(Some(1)).is_ok());
        assert!(check_wait(Some(600)).is_ok());
        assert!(check_wait(Some(601)).is_err());
    }

    #[test]
    fn notify_lists() -> Result<()> {
        assert_eq!(
            Notify::parse_list("show_dismissed,speech_finished,show_dismissed")?,
            vec![Notify::SpeechFinished, Notify::ShowDismissed]
        );
        assert_eq!(
            Notify::parse_list("shown,speech_finished")?,
            vec![Notify::SpeechFinished, Notify::Shown]
        );
        assert!(Notify::parse_list("")?.is_empty());
        let e = Notify::parse_list("everything").err();
        assert!(e.is_some_and(|e| {
            e.message()
                .contains("speech_finished, show_dismissed, shown")
        }));
        assert_eq!(
            Notify::ALL.map(Notify::as_str),
            ["speech_finished", "show_dismissed", "shown"]
        );
        assert_eq!(
            serde_json::to_value(Notify::Shown).ok(),
            Some(serde_json::json!("shown"))
        );
        Ok(())
    }

    fn secs(flag: &str, raw: &str) -> Option<u64> {
        parse_duration(flag, raw).ok().map(|d| d.as_secs())
    }

    #[test]
    fn duration_vectors() {
        for (raw, want) in [
            ("60", 60),
            (" 90s ", 90),
            ("15m", 900),
            ("2h", 7200),
            ("1d", 86_400),
            ("1h30m", 5400),
            ("2d12h", 216_000),
            ("0", 0),
            ("1d2h3m4s", 93_784),
            ("9999999s", 9_999_999),
            ("31536000", 31_536_000),
        ] {
            assert_eq!(secs("--in", raw), Some(want), "{raw:?}");
        }
        for raw in [
            "",
            "   ",
            "1.5h",
            "90S",
            "1m1h",
            "1h1h",
            "1 h",
            "-5m",
            "5min",
            "12345678s",
            "1w",
            "h",
            "d1",
            "1hh",
            "1h 30m",
            "+5m",
            "٣s",
            "99999999999999999999",
        ] {
            let e = parse_duration("--expires-in", raw).err();
            assert!(
                e.as_ref()
                    .is_some_and(|e| *e.code() == ErrorCode::InvalidInput
                        && e.hint().is_some()
                        && e.details().is_some_and(|d| d["field"] == "--expires-in"
                            && d["value"] == raw
                            && d["expected"] == DURATION_EXPECTED)),
                "{raw:?} must be refused: {e:?}"
            );
        }
    }

    #[test]
    fn duration_ranges() {
        let ok = |r: Result<Duration>| r.is_ok();
        assert!(!ok(parse_expires_in("9s")));
        assert!(ok(parse_expires_in("10s")));
        assert!(ok(parse_expires_in("7d")));
        assert!(!ok(parse_expires_in("7d1s")));
        assert!(!ok(parse_expires_in("0")));
        assert!(
            ok(parse_expires_in("60")),
            "0.1.1's plain seconds still work"
        );
        assert!(!ok(parse_schedule_in("0s")));
        assert!(!ok(parse_schedule_in("0")));
        assert!(ok(parse_schedule_in("1s")));
        assert!(ok(parse_schedule_in("365d")));
        assert!(!ok(parse_schedule_in("365d1s")));
        for (e, field) in [
            (parse_expires_in("9s").err(), "--expires-in"),
            (parse_schedule_in("366d").err(), "--in"),
            (parse_schedule_in("soon").err(), "--in"),
        ] {
            assert!(
                e.as_ref()
                    .is_some_and(|e| *e.code() == ErrorCode::InvalidInput
                        && e.exit_code().code() == 2
                        && e.hint().is_some()
                        && e.details().is_some_and(|d| d["field"] == field)),
                "{e:?}"
            );
        }
        let e = parse_schedule_in("366d").err();
        assert!(
            e.is_some_and(|e| e.message() == "--in 366d is out of range; it must be 1 s – 365 d")
        );
    }

    #[test]
    fn tz_names() {
        assert!(check_tz_name("Asia/Kolkata").is_ok());
        assert!(check_tz_name("America/Argentina/Buenos_Aires").is_ok());
        assert!(check_tz_name("Etc/GMT+5").is_ok());
        assert!(check_tz_name("UTC").is_ok());
        assert!(check_tz_name(&"a".repeat(64)).is_ok());
        assert!(check_tz_name(&"a".repeat(65)).is_err());
        assert!(check_tz_name("").is_err());
        assert!(check_tz_name("Asia Kolkata").is_err());
        let e = check_tz_name("Asia/Kolkata\n").err();
        assert!(e.is_some_and(|e| e.details().is_some_and(|d| d["field"] == "--tz")));
    }

    fn flags_err(f: SendFlags) -> Option<(ErrorCode, String, Option<String>)> {
        check_flags(f).err().map(|e| {
            (
                e.code().clone(),
                e.message().to_owned(),
                e.hint().map(str::to_owned),
            )
        })
    }

    #[test]
    fn flag_rules() {
        let code = |f: SendFlags| check_flags(f).err().map(|e| e.code().clone());
        assert_eq!(code(SendFlags::default()), Some(ErrorCode::NothingToSend));
        assert_eq!(
            code(SendFlags {
                show: true,
                ask: true,
                ..SendFlags::default()
            }),
            Some(ErrorCode::ConflictingFlags)
        );
        assert_eq!(
            code(SendFlags {
                show: true,
                voice: true,
                ..SendFlags::default()
            }),
            Some(ErrorCode::ConflictingFlags)
        );
        assert_eq!(
            code(SendFlags {
                speak: true,
                expires_in: true,
                ..SendFlags::default()
            }),
            None,
            "--expires-in now applies to every kind"
        );
        assert_eq!(
            code(SendFlags {
                show: true,
                expires_at: true,
                tz: true,
                ..SendFlags::default()
            }),
            None
        );
        assert_eq!(
            code(SendFlags {
                show: true,
                wait: true,
                ..SendFlags::default()
            }),
            Some(ErrorCode::ConflictingFlags)
        );
        let e = flags_err(SendFlags {
            speak: true,
            ask: true,
            duration: true,
            ..SendFlags::default()
        });
        assert_eq!(
            e.and_then(|e| e.2).as_deref(),
            Some("use --expires-in <DURATION> or --expires-at <DATETIME> to bound an ask")
        );
        assert_eq!(
            code(SendFlags {
                speak: true,
                ask: true,
                wait: true,
                expires_in: true,
                voice: true,
                ..SendFlags::default()
            }),
            None
        );
        assert_eq!(
            code(SendFlags {
                speak: true,
                show: true,
                duration: true,
                lang: true,
                ..SendFlags::default()
            }),
            None
        );
        assert_eq!(
            code(SendFlags {
                speak: true,
                schedule_at: true,
                expires_at: true,
                tz: true,
                replace: true,
                ..SendFlags::default()
            }),
            None
        );
        assert_eq!(
            code(SendFlags {
                ask: true,
                schedule_in: true,
                ..SendFlags::default()
            }),
            None
        );
    }

    #[test]
    fn replace_combines_with_everything() {
        for base in [
            SendFlags {
                speak: true,
                ..SendFlags::default()
            },
            SendFlags {
                show: true,
                duration: true,
                ..SendFlags::default()
            },
            SendFlags {
                ask: true,
                wait: true,
                expires_in: true,
                ..SendFlags::default()
            },
            SendFlags {
                ask: true,
                schedule_at: true,
                tz: true,
                expires_at: true,
                ..SendFlags::default()
            },
            SendFlags {
                speak: true,
                schedule_in: true,
                ..SendFlags::default()
            },
        ] {
            assert!(check_flags(base).is_ok(), "{base:?}");
            assert!(
                check_flags(SendFlags {
                    replace: true,
                    ..base
                })
                .is_ok(),
                "{base:?}"
            );
        }
    }

    #[test]
    fn new_conflicts_have_exact_messages() {
        let table: [(SendFlags, &str, &str); 5] = [
            (
                SendFlags {
                    speak: true,
                    expires_in: true,
                    expires_at: true,
                    ..SendFlags::default()
                },
                "--expires-in and --expires-at cannot be combined; give one deadline",
                "--expires-in 15m, or --expires-at 2026-09-27T18:00",
            ),
            (
                SendFlags {
                    speak: true,
                    schedule_in: true,
                    schedule_at: true,
                    ..SendFlags::default()
                },
                "--in and --at cannot be combined; a send is scheduled once",
                "--in 2h, or --at 2026-09-27T18:00",
            ),
            (
                SendFlags {
                    speak: true,
                    schedule_in: true,
                    expires_in: true,
                    ..SendFlags::default()
                },
                "a scheduled send (--in/--at) takes --expires-at, not --expires-in: its clock would start now, not at the due time",
                "--expires-at <DATETIME> (after the due time)",
            ),
            (
                SendFlags {
                    ask: true,
                    schedule_at: true,
                    wait: true,
                    ..SendFlags::default()
                },
                "--wait cannot be used with --in or --at: nobody waits for a scheduled ask",
                "drop --wait; the answer arrives as peek.ask.answered",
            ),
            (
                SendFlags {
                    speak: true,
                    tz: true,
                    schedule_in: true,
                    ..SendFlags::default()
                },
                "--tz applies only to --at and --expires-at",
                "drop --tz, or add --at <DATETIME>",
            ),
        ];
        for (f, msg, hint) in table {
            assert_eq!(
                flags_err(f),
                Some((
                    ErrorCode::ConflictingFlags,
                    msg.to_owned(),
                    Some(hint.to_owned())
                )),
                "{f:?}"
            );
        }
        // --at + --expires-in is the scheduled/expires-in rule too.
        assert!(
            flags_err(SendFlags {
                speak: true,
                schedule_at: true,
                expires_in: true,
                ..SendFlags::default()
            })
            .is_some_and(|e| e.1.starts_with("a scheduled send"))
        );
        // --tz alone.
        assert!(
            flags_err(SendFlags {
                speak: true,
                tz: true,
                ..SendFlags::default()
            })
            .is_some_and(|e| e.1 == "--tz applies only to --at and --expires-at")
        );
    }
}
