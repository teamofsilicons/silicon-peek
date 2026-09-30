//! `ElevenLabs` v4 voices are multilingual. George is the default.

use silicon_peek_client::{ipc::cli::SpeechStatus, schema::send::DEFAULT_TTS_VOICE};
use std::collections::BTreeMap;
use whatlang::Lang;

/// How a send will be spoken.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoicePlan {
    /// The speech is ready to synthesize.
    pub status: SpeechStatus,
    /// `ElevenLabs` voice ID.
    pub model: Option<String>,
    /// Explicit or detected primary language, when known.
    pub language: Option<String>,
}

fn iso(lang: Lang) -> &'static str {
    match lang {
        Lang::Eng => "en",
        Lang::Spa => "es",
        Lang::Deu => "de",
        Lang::Fra => "fr",
        Lang::Nld => "nl",
        Lang::Ita => "it",
        Lang::Jpn => "ja",
        Lang::Por => "pt",
        Lang::Hin => "hi",
        Lang::Cmn => "zh",
        Lang::Kor => "ko",
        Lang::Rus => "ru",
        Lang::Ara => "ar",
        _ => "und",
    }
}

/// Resolve CLI/config voice, language preference, and the Carbon's overrides.
#[must_use]
pub fn plan(
    text: &str,
    explicit_voice: Option<&str>,
    forced_lang: Option<&str>,
    fallback_lang: Option<&str>,
    overrides: &BTreeMap<String, String>,
) -> VoicePlan {
    let language = forced_lang
        .map(str::to_owned)
        .or_else(|| {
            whatlang::detect(text)
                .filter(whatlang::Info::is_reliable)
                .map(|info| iso(info.lang()))
                .filter(|lang| *lang != "und")
                .map(str::to_owned)
        })
        .or_else(|| fallback_lang.map(str::to_owned));
    let voice = explicit_voice
        .or_else(|| {
            language
                .as_ref()
                .and_then(|lang| overrides.get(lang))
                .map(String::as_str)
        })
        .unwrap_or(DEFAULT_TTS_VOICE);
    VoicePlan {
        status: SpeechStatus::Pending,
        model: Some(voice.to_owned()),
        language,
    }
}

/// Approximate progress before the actual audio frame count arrives.
#[must_use]
pub fn estimated_frames(chars: usize) -> u64 {
    let chars = u64::try_from(chars).unwrap_or(u64::MAX);
    chars.saturating_mul(24_000) / 14
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn multilingual_voice_preferences() {
        let overrides = BTreeMap::from([("hi".to_owned(), "DtsPFCrhbCbbJkwZsb3d".to_owned())]);
        let p = plan("नमस्ते", None, Some("hi"), None, &overrides);
        assert_eq!(p.model.as_deref(), Some("DtsPFCrhbCbbJkwZsb3d"));
        assert_eq!(p.status, SpeechStatus::Pending);
        assert_eq!(
            plan("hello", Some("voice_custom"), None, None, &overrides)
                .model
                .as_deref(),
            Some("voice_custom")
        );
        assert_eq!(
            plan("hello", None, None, None, &overrides).model.as_deref(),
            Some(DEFAULT_TTS_VOICE)
        );
        assert_eq!(estimated_frames(14), 24_000);
    }
}
