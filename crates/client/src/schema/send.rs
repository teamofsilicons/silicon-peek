//! Validators for `peek send` options (BLUEPRINT §7.4) and the TTS voice and
//! language settings shared with `peek config` (§7.3).

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{Lines, check_text, limits};
use crate::error::{Error, ErrorCode, Result};

/// Languages Deepgram Aura-2 speaks (BLUEPRINT §8.7).
pub const TTS_LANGUAGES: [&str; 7] = ["en", "es", "de", "fr", "nl", "it", "ja"];

/// Opt-in notifications (`--notify`, config `notify`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Notify {
    /// Send `peek.speech.finished` when a `--speak` finishes or is stopped.
    SpeechFinished,
    /// Send `peek.show.dismissed` when the Carbon closes a `--show` early.
    ShowDismissed,
}

impl Notify {
    /// Every value.
    pub const ALL: [Notify; 2] = [Notify::SpeechFinished, Notify::ShowDismissed];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SpeechFinished => "speech_finished",
            Self::ShowDismissed => "show_dismissed",
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
            other => Err(Error::invalid_input(format!(
                "`{other}` is not a notification; allowed: speech_finished, show_dismissed"
            ))),
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
            e.with_hint("Deepgram Aura speaks at most 2000 characters per request; split it over several sends")
        } else {
            e
        }
    })
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

/// Validates a Deepgram voice: `^aura-2-[a-z]+-(en|es|de|fr|nl|it|ja)$`.
///
/// # Errors
/// `invalid_input` with an example.
pub fn check_voice(voice: &str) -> Result<()> {
    let ok = voice
        .strip_prefix("aura-2-")
        .and_then(|rest| rest.rsplit_once('-'))
        .is_some_and(|(name, lang)| {
            !name.is_empty()
                && name.bytes().all(|b| b.is_ascii_lowercase())
                && TTS_LANGUAGES.contains(&lang)
        });
    if ok {
        Ok(())
    } else {
        Err(Error::invalid_input(format!(
            "voice `{voice}` is not an Aura-2 voice; expected aura-2-<name>-<{}>",
            TTS_LANGUAGES.join("|")
        ))
        .with_hint(
            "for example aura-2-thalia-en; see https://developers.deepgram.com/docs/tts-models",
        ))
    }
}

/// The language of an Aura-2 voice (`aura-2-thalia-en` → `en`).
#[must_use]
pub fn voice_language(voice: &str) -> Option<&str> {
    voice.rsplit_once('-').map(|(_, l)| l)
}

/// Validates and normalizes a BCP 47 primary language subtag (`EN` → `en`).
///
/// # Errors
/// `invalid_input` unless the value is 2–3 ASCII letters.
pub fn normalize_language(lang: &str) -> Result<String> {
    if (2..=3).contains(&lang.len()) && lang.bytes().all(|b| b.is_ascii_alphabetic()) {
        Ok(lang.to_ascii_lowercase())
    } else {
        Err(Error::invalid_input(format!(
            "language `{lang}` is not a BCP 47 primary subtag; use two or three letters such as `en` or `ja`"
        )))
    }
}

fn bounded_secs(flag: &str, secs: u64, min: u64, max: u64) -> Result<Duration> {
    if (min..=max).contains(&secs) {
        Ok(Duration::from_secs(secs))
    } else {
        Err(Error::invalid_input(format!(
            "{flag} {secs} is out of range; it must be {min}–{max} seconds"
        ))
        .with_details(json!({"flag": flag, "min": min, "max": max, "actual": secs})))
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
    /// `--lang` given.
    pub lang: bool,
    /// `--duration` given.
    pub duration: bool,
    /// `--expires-in` given.
    pub expires_in: bool,
    /// `--wait` given.
    pub wait: bool,
}

/// Enforces the flag combination rules: at least one of speak/show/ask;
/// show and ask are exclusive; voice/lang need speak; duration needs show;
/// expires-in and wait need ask.
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
    if (f.voice || f.lang) && !f.speak {
        return conflict(
            "--voice and --lang apply only to --speak",
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
            "use --expires-in <SECS> to bound an ask",
        );
    }
    if f.expires_in && !f.ask {
        return conflict(
            "--expires-in applies only to --ask",
            "drop --expires-in, or add --ask",
        );
    }
    if f.wait && !f.ask {
        return conflict("--wait applies only to --ask", "drop --wait, or add --ask");
    }
    Ok(())
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
        assert!(check_voice("aura-2-thalia-en").is_ok());
        assert!(check_voice("aura-2-izanami-ja").is_ok());
        assert!(check_voice("aura-2-thalia-pt").is_err());
        assert!(check_voice("aura-2--en").is_err());
        assert!(check_voice("aura-1-thalia-en").is_err());
        assert!(check_voice("aura-2-Thalia-en").is_err());
        assert_eq!(voice_language("aura-2-celeste-es"), Some("es"));
        assert_eq!(normalize_language("EN").ok().as_deref(), Some("en"));
        assert!(normalize_language("e").is_err());
        assert!(normalize_language("english").is_err());
        assert!(normalize_language("en-US").is_err());
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
        assert!(Notify::parse_list("")?.is_empty());
        assert!(Notify::parse_list("everything").is_err());
        Ok(())
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
            Some(ErrorCode::ConflictingFlags)
        );
        assert_eq!(
            code(SendFlags {
                show: true,
                wait: true,
                ..SendFlags::default()
            }),
            Some(ErrorCode::ConflictingFlags)
        );
        assert_eq!(
            code(SendFlags {
                ask: true,
                duration: true,
                ..SendFlags::default()
            }),
            Some(ErrorCode::ConflictingFlags)
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
    }
}
