import Foundation
import PeekCore

/// ElevenLabs v4 voices verified through Deepgram. Every voice is multilingual.
public enum VoiceCatalog {
    public struct Voice: Hashable, Sendable, Identifiable {
        public let id: String
        public let name: String
        public var label: String { name }
    }

    public static let languages = DefaultVoices.supportedLanguages

    public static func languageName(_ code: String) -> String {
        Locale(identifier: "en").localizedString(forLanguageCode: code) ?? code
    }

    public static func voices(for language: String) -> [Voice] {
        all.sorted { lhs, rhs in
            let preferred = defaultVoice(for: language)
            if (lhs.id == preferred) != (rhs.id == preferred) { return lhs.id == preferred }
            return lhs.name < rhs.name
        }
    }

    public static func voice(id: String) -> Voice? { all.first { $0.id == id } }
    public static func defaultVoice(for language: String) -> String? { DefaultVoices.byLanguage[language] }

    // Deepgram playground voice IDs plus George, verified with eleven_v4.
    public static let all: [Voice] = [
        ("DtsPFCrhbCbbJkwZsb3d", "Piper"),
        ("UgBBYS2sOqTuMpoF3BR0", "Mark"),
        ("cgSgspJ2msm6clMCkdW9", "Jessica"),
        ("onwK4e9ZLuTAKqWW03F9", "Daniel"),
        ("jBlmi27XRORxjPquUeCh", "Brian"),
        ("TC0Zp7WVFzhA8zpTlRqV", "Aria Bloom"),
        ("D6MRWCKoavI2xUJXmaCb", "Jennifer"),
        ("gdTrLNuwWUaxC0z5n1j7", "Jerry"),
        ("IPgYtHTNLjC7Bq7IPHrm", "Alexandre Boutin"),
        ("F1toM6PcP54s45kOOAyV", "Mademoiselle French"),
        ("aTTiK3YzK3dXETpuDE2h", "Ben"),
        ("mDRP1h6KfUD1XAUJxqr0", "Doreen Pelz"),
        ("CiwzbDpaN3pQXjTgx3ML", "Aida"),
        ("Fahco4VZzobUeiPqni1S", "Archer - Conversational"),
        ("j210dv0vWm7fCknyQpbA", "Hinata"),
        ("8EkOjt4xTPGMclNlh1pk", "Morioki"),
        ("JBFqnCBsd6RMkjVDRZzb", "George"),
    ].map { Voice(id: $0.0, name: $0.1) }
}

/// Choices for settings.json `stt_language` (BLUEPRINT §8.7).
///
/// `auto` supplies the Mac's preferred languages as OpenAI recognition hints;
/// a selected BCP 47 tag supplies a single hint. Detection still happens at OpenAI.
/// The list is a convenience picker, not a provider language whitelist.
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
