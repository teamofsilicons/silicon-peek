//! Which Aura-2 voice speaks a `--speak` (BLUEPRINT §8.7, §1.9.3):
//! `--voice` (or config `voice`, merged by the CLI) wins; otherwise the text's
//! language is detected with `whatlang` and mapped to the per-language default
//! (or the Carbon's `voice_defaults` override). Languages Aura-2 does not speak
//! get no speech at all: the text is shown as a pill instead.

use std::collections::BTreeMap;

use silicon_peek_client::{
    ipc::cli::SpeechStatus,
    schema::send::{TTS_LANGUAGES, voice_language},
};
use whatlang::{Lang, Script};

/// The default voice per language (§8.7 table).
#[must_use]
pub fn default_voice(lang: &str, mixed_en_es: bool) -> Option<&'static str> {
    Some(match lang {
        "en" | "es" if mixed_en_es => "aura-2-selena-es",
        "en" => "aura-2-thalia-en",
        "es" => "aura-2-celeste-es",
        "de" => "aura-2-viktoria-de",
        "fr" => "aura-2-agathe-fr",
        "nl" => "aura-2-rhea-nl",
        "it" => "aura-2-livia-it",
        "ja" => "aura-2-izanami-ja",
        _ => return None,
    })
}

/// How a send will be spoken.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoicePlan {
    /// `pending` (to synthesize) or `unsupported_language`.
    pub status: SpeechStatus,
    /// The Aura-2 model, when speaking.
    pub model: Option<String>,
    /// The language (ISO 639-1), when known.
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

const EN_MARKERS: [&str; 20] = [
    "the", "and", "is", "are", "you", "your", "this", "that", "with", "for", "what", "please",
    "thanks", "hello", "okay", "we", "it's", "i'm", "going", "have",
];
const ES_MARKERS: [&str; 20] = [
    "el", "la", "los", "las", "y", "es", "que", "de", "para", "por", "con", "una", "un", "hola",
    "gracias", "está", "estoy", "vamos", "pero", "muy",
];

/// Whether the text mixes English and Spanish enough to warrant a
/// codeswitching voice (`aura-2-selena-es`).
fn mixes_en_es(text: &str) -> bool {
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect();
    let en = words
        .iter()
        .filter(|w| EN_MARKERS.contains(&w.as_str()))
        .count();
    let es = words
        .iter()
        .filter(|w| ES_MARKERS.contains(&w.as_str()))
        .count();
    en >= 2 && es >= 2
}

/// Plans the voice for `text`.
///
/// - `explicit_voice`: `--voice` or config `voice` (already validated).
/// - `forced_lang`: `--lang` or config `language`.
/// - `fallback_lang`: the home's mirrored config language, used only when
///   detection is unreliable.
/// - `overrides`: the Carbon's `voice_defaults`.
#[must_use]
pub fn plan(
    text: &str,
    explicit_voice: Option<&str>,
    forced_lang: Option<&str>,
    fallback_lang: Option<&str>,
    overrides: &BTreeMap<String, String>,
) -> VoicePlan {
    if let Some(v) = explicit_voice {
        return VoicePlan {
            status: SpeechStatus::Pending,
            model: Some(v.to_owned()),
            language: voice_language(v).map(str::to_owned),
        };
    }
    let lang = match forced_lang {
        Some(l) => l.to_ascii_lowercase(),
        None => detect(text, fallback_lang),
    };
    if !TTS_LANGUAGES.contains(&lang.as_str()) {
        return VoicePlan {
            status: SpeechStatus::UnsupportedLanguage,
            model: None,
            language: (lang != "und").then_some(lang),
        };
    }
    let mixed = matches!(lang.as_str(), "en" | "es") && mixes_en_es(text);
    let model = if mixed {
        default_voice(&lang, true).map(str::to_owned)
    } else {
        overrides
            .get(&lang)
            .cloned()
            .or_else(|| default_voice(&lang, false).map(str::to_owned))
    };
    VoicePlan {
        status: SpeechStatus::Pending,
        model,
        language: Some(lang),
    }
}

/// Detects the language: a reliable `whatlang` result wins; an unreliable
/// one in Latin script falls back to `fallback` (or English); an unreliable
/// non-Latin result keeps its guess (Japanese kana is unambiguous).
fn detect(text: &str, fallback: Option<&str>) -> String {
    let Some(info) = whatlang::detect(text) else {
        return fallback.unwrap_or("en").to_owned();
    };
    let guess = iso(info.lang());
    if info.is_reliable() {
        return guess.to_owned();
    }
    match info.script() {
        Script::Latin => fallback.filter(|f| TTS_LANGUAGES.contains(f)).map_or_else(
            || {
                if TTS_LANGUAGES.contains(&guess) && info.confidence() >= 0.5 {
                    guess.to_owned()
                } else {
                    "en".to_owned()
                }
            },
            str::to_owned,
        ),
        Script::Hiragana | Script::Katakana => "ja".to_owned(),
        _ => guess.to_owned(),
    }
}

/// Frames Aura-2 needs for `chars` characters (≈ 14 characters per second
/// at 24 kHz), the UI's progress estimate before `tts.end`.
#[must_use]
pub fn estimated_frames(chars: usize) -> u64 {
    let chars = u64::try_from(chars).unwrap_or(u64::MAX);
    chars.saturating_mul(24_000) / 14
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none() -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    #[test]
    fn explicit_and_forced() {
        let p = plan("hello", Some("aura-2-apollo-en"), None, None, &none());
        assert_eq!(p.model.as_deref(), Some("aura-2-apollo-en"));
        assert_eq!(p.language.as_deref(), Some("en"));
        let p = plan("bonjour", None, Some("fr"), None, &none());
        assert_eq!(p.model.as_deref(), Some("aura-2-agathe-fr"));
        let p = plan("namaste", None, Some("hi"), None, &none());
        assert_eq!(p.status, SpeechStatus::UnsupportedLanguage);
        assert_eq!(p.model, None);
    }

    #[test]
    fn detection_table() {
        let cases = [
            (
                "The build finished and all tests passed. Deploying to staging now.",
                "aura-2-thalia-en",
            ),
            (
                "La compilación terminó y todas las pruebas pasaron correctamente esta mañana.",
                "aura-2-celeste-es",
            ),
            (
                "Der Build ist fertig und alle Tests waren erfolgreich, wir können jetzt deployen.",
                "aura-2-viktoria-de",
            ),
            (
                "La compilation est terminée et tous les tests sont passés avec succès ce matin.",
                "aura-2-agathe-fr",
            ),
            (
                "De build is klaar en alle tests zijn geslaagd, we kunnen nu gaan uitrollen.",
                "aura-2-rhea-nl",
            ),
            (
                "La compilazione è finita e tutti i test sono passati con successo stamattina.",
                "aura-2-livia-it",
            ),
            (
                "ビルドが完了しました。すべてのテストに合格しました。",
                "aura-2-izanami-ja",
            ),
            ("Build done", "aura-2-thalia-en"),
        ];
        for (text, voice) in cases {
            let p = plan(text, None, None, None, &none());
            assert_eq!(p.status, SpeechStatus::Pending, "{text}");
            assert_eq!(p.model.as_deref(), Some(voice), "{text}");
        }
    }

    #[test]
    fn unsupported_languages_are_not_spoken() {
        for text in [
            "बिल्ड पूरा हो गया है और सभी परीक्षण सफल रहे हैं, अब हम आगे बढ़ सकते हैं।",
            "Сборка завершена, и все тесты успешно пройдены, можно выкатывать.",
            "빌드가 완료되었고 모든 테스트를 통과했습니다. 이제 배포할 수 있습니다.",
        ] {
            let p = plan(text, None, None, None, &none());
            assert_eq!(p.status, SpeechStatus::UnsupportedLanguage, "{text}");
        }
    }

    #[test]
    fn mixing_and_overrides() {
        let p = plan(
            "Hola, the meeting es a las cinco, please confirm y gracias por todo.",
            None,
            None,
            None,
            &none(),
        );
        assert_eq!(p.model.as_deref(), Some("aura-2-selena-es"));
        let mut o = BTreeMap::new();
        o.insert("en".to_owned(), "aura-2-apollo-en".to_owned());
        let p = plan(
            "The build finished and all tests passed.",
            None,
            None,
            None,
            &o,
        );
        assert_eq!(p.model.as_deref(), Some("aura-2-apollo-en"));
        let p = plan("ok", None, None, Some("de"), &none());
        assert_eq!(p.model.as_deref(), Some("aura-2-viktoria-de"));
        assert_eq!(estimated_frames(14), 24_000);
    }
}
