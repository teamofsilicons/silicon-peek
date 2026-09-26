import Foundation
import PeekCore

/// The Deepgram Aura-2 voices peek can speak with (notes/speech §1.4, BLUEPRINT §8.7).
///
/// peekd picks the voice for a send: `--voice`, else the Silicon's config `voice`, else the
/// Carbon's default for the detected language from settings.json `voice_defaults`, which is what
/// Settings › Voice edits. Every id here matches `aura-2-<name>-<lang>`
/// (``DefaultVoices/isValidVoice(_:)``), and the defaults are ``DefaultVoices/byLanguage``.
public enum VoiceCatalog {
    public struct Voice: Hashable, Sendable, Identifiable {
        public enum Gender: String, Sendable {
            case feminine = "F"
            case masculine = "M"
        }

        /// The model id, e.g. `aura-2-thalia-en`.
        public let id: String
        /// The voice's name, e.g. `Thalia`.
        public let name: String
        public let language: String
        public let gender: Gender
        /// Accent or character note from Deepgram's list (`British`, `Mexico`, `mature`, …).
        public let note: String?
        /// Deepgram marks a few voices per language as featured.
        public let featured: Bool
        /// English–Spanish code-switching voice (Spanish only).
        public let codeSwitching: Bool

        /// "Thalia · F · featured" style label for pickers.
        public var label: String {
            var parts = [name, gender.rawValue]
            if let note { parts.append(note) }
            if codeSwitching { parts.append("EN–ES code-switching") }
            if featured { parts.append("featured") }
            return parts.joined(separator: " · ")
        }
    }

    /// Languages Aura-2 speaks, in the order Settings lists them.
    public static let languages: [String] = DefaultVoices.supportedLanguages

    /// English display name of a supported language (`en` → `English`).
    public static func languageName(_ code: String) -> String {
        switch code {
        case "en": "English"
        case "es": "Spanish"
        case "de": "German"
        case "fr": "French"
        case "nl": "Dutch"
        case "it": "Italian"
        case "ja": "Japanese"
        default: Locale(identifier: "en").localizedString(forLanguageCode: code) ?? code
        }
    }

    /// Every voice of `language`: featured voices first, then alphabetical.
    public static func voices(for language: String) -> [Voice] {
        all.filter { $0.language == language }
            .sorted { lhs, rhs in
                if lhs.featured != rhs.featured { return lhs.featured }
                return lhs.name < rhs.name
            }
    }

    public static func voice(id: String) -> Voice? { all.first { $0.id == id } }

    /// The default voice for `language` (``DefaultVoices/byLanguage``).
    public static func defaultVoice(for language: String) -> String? { DefaultVoices.byLanguage[language] }

    // MARK: Data (notes/speech §1.4; F = feminine, M = masculine, accent American unless noted)

    /// All 90 Aura-2 voices.
    public static let all: [Voice] = english + spanish + dutch + french + german + italian + japanese

    private static func make(_ language: String, _ entries: [(String, Voice.Gender, String?)],
                             featured: Set<String> = [], codeSwitching: Set<String> = []) -> [Voice] {
        entries.map { name, gender, note in
            Voice(
                id: "aura-2-\(name)-\(language)", name: name.prefix(1).uppercased() + name.dropFirst(), language: language,
                gender: gender, note: note, featured: featured.contains(name), codeSwitching: codeSwitching.contains(name))
        }
    }

    private static let english = make(
        "en",
        [
            ("amalthea", .feminine, "Filipino"), ("andromeda", .feminine, nil), ("apollo", .masculine, nil),
            ("arcas", .masculine, nil), ("aries", .masculine, nil), ("asteria", .feminine, nil),
            ("athena", .feminine, "mature"), ("atlas", .masculine, "mature"), ("aurora", .feminine, nil),
            ("callista", .feminine, nil), ("cora", .feminine, nil), ("cordelia", .feminine, nil),
            ("delia", .feminine, nil), ("draco", .masculine, "British"), ("electra", .feminine, nil),
            ("harmonia", .feminine, nil), ("helena", .feminine, nil), ("hera", .feminine, nil),
            ("hermes", .masculine, nil), ("hyperion", .masculine, "Australian"), ("iris", .feminine, nil),
            ("janus", .feminine, "Southern"), ("juno", .feminine, nil), ("jupiter", .masculine, nil),
            ("luna", .feminine, nil), ("mars", .masculine, nil), ("minerva", .feminine, nil),
            ("neptune", .masculine, nil), ("odysseus", .masculine, nil), ("ophelia", .feminine, nil),
            ("orion", .masculine, nil), ("orpheus", .masculine, nil), ("pandora", .feminine, "British"),
            ("phoebe", .feminine, nil), ("pluto", .masculine, nil), ("saturn", .masculine, nil),
            ("selene", .feminine, nil), ("thalia", .feminine, nil), ("theia", .feminine, "Australian"),
            ("vesta", .feminine, nil), ("zeus", .masculine, nil),
        ],
        featured: ["thalia", "andromeda", "helena", "apollo", "arcas", "aries"])

    private static let spanish = make(
        "es",
        [
            ("sirio", .masculine, "Mexico"), ("nestor", .masculine, "Spain"), ("carina", .feminine, "Spain"),
            ("celeste", .feminine, "Colombia"), ("alvaro", .masculine, "Spain"), ("diana", .feminine, "Spain"),
            ("aquila", .masculine, "Latin America"), ("selena", .feminine, "Latin America"),
            ("estrella", .feminine, "Mexico"), ("javier", .masculine, "Mexico"), ("agustina", .feminine, "Spain"),
            ("antonia", .feminine, "Argentina"), ("gloria", .feminine, "Colombia"), ("luciano", .masculine, "Mexico"),
            ("olivia", .feminine, "Mexico"), ("silvia", .feminine, "Spain"), ("valerio", .masculine, "Mexico"),
        ],
        featured: ["celeste", "estrella", "nestor"],
        codeSwitching: ["carina", "diana", "aquila", "selena", "javier"])

    private static let dutch = make(
        "nl",
        [
            ("beatrix", .feminine, nil), ("daphne", .feminine, nil), ("cornelia", .feminine, nil),
            ("sander", .masculine, nil), ("hestia", .feminine, nil), ("lars", .masculine, nil),
            ("roman", .masculine, nil), ("rhea", .feminine, nil), ("leda", .feminine, nil),
        ],
        featured: ["rhea", "sander", "beatrix"])

    private static let french = make("fr", [("agathe", .feminine, nil), ("hector", .masculine, nil)])

    private static let german = make(
        "de",
        [
            ("elara", .feminine, nil), ("aurelia", .feminine, nil), ("lara", .feminine, nil),
            ("julius", .masculine, nil), ("fabian", .masculine, "mature"), ("kara", .feminine, nil),
            ("viktoria", .feminine, nil),
        ],
        featured: ["julius", "viktoria"])

    private static let italian = make(
        "it",
        [
            ("melia", .feminine, nil), ("elio", .masculine, nil), ("flavio", .masculine, nil), ("maia", .feminine, nil),
            ("cinzia", .feminine, "mature"), ("cesare", .masculine, nil), ("livia", .feminine, nil),
            ("dionisio", .masculine, nil), ("demetra", .feminine, nil),
        ],
        featured: ["livia", "dionisio"])

    private static let japanese = make(
        "ja",
        [
            ("uzume", .feminine, nil), ("ebisu", .masculine, nil), ("fujin", .masculine, nil),
            ("izanami", .feminine, nil), ("ama", .feminine, nil),
        ],
        featured: ["fujin", "izanami"])
}

/// Choices for settings.json `stt_language` (BLUEPRINT §8.7).
///
/// `auto` sends peekd `Locale.preferredLanguages`: one language becomes Deepgram's `language=`,
/// several become repeated `detect_language=`. A fixed BCP 47 tag overrides that. The list below
/// is the common part of Nova-3's monolingual languages; any other valid tag can be typed in.
public enum STTLanguageChoices {
    public static let auto = "auto"

    public struct Choice: Hashable, Sendable, Identifiable {
        public let tag: String
        public let name: String
        public var id: String { tag }
    }

    public static let common: [Choice] = [
        ("en", "English"), ("en-US", "English (United States)"), ("en-GB", "English (United Kingdom)"),
        ("en-AU", "English (Australia)"), ("en-IN", "English (India)"), ("es", "Spanish"),
        ("es-419", "Spanish (Latin America)"), ("fr", "French"), ("fr-CA", "French (Canada)"), ("de", "German"),
        ("it", "Italian"), ("pt", "Portuguese"), ("pt-BR", "Portuguese (Brazil)"), ("nl", "Dutch"),
        ("ja", "Japanese"), ("ko", "Korean"), ("hi", "Hindi"), ("ru", "Russian"), ("uk", "Ukrainian"),
        ("pl", "Polish"), ("tr", "Turkish"), ("sv", "Swedish"), ("da", "Danish"), ("nb", "Norwegian Bokmål"),
        ("fi", "Finnish"), ("id", "Indonesian"), ("vi", "Vietnamese"),
    ].map { Choice(tag: $0.0, name: $0.1) }

    /// Whether `tag` is one of ``common``.
    public static func isCommon(_ tag: String) -> Bool { common.contains { $0.tag == tag } }

    /// What `auto` resolves to on this Mac, e.g. `en-US, hi-IN`.
    public static func automaticDescription(preferred: [String] = Locale.preferredLanguages) -> String {
        preferred.isEmpty ? "the system language" : preferred.prefix(4).joined(separator: ", ")
    }

    /// Validates a typed tag; returns the error to show, or nil when `tag` is acceptable.
    public static func problem(with tag: String) -> String? {
        let trimmed = tag.trimmingCharacters(in: .whitespaces)
        if trimmed.isEmpty { return "Type a BCP 47 language tag such as \"pt-BR\", or choose Automatic." }
        if trimmed == auto || PeekSettings.isLanguageTag(trimmed) { return nil }
        return "\"\(trimmed)\" is not a BCP 47 tag. Use a lowercase 2–3 letter language, optionally followed by "
            + "subtags such as a region: \"en\", \"pt-BR\", \"es-419\"."
    }
}
