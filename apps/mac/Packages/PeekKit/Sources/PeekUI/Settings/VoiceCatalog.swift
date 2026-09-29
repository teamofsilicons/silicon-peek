import Foundation
import PeekCore

/// Google's prebuilt multilingual voices. Per-language preferences share this catalog.
public enum VoiceCatalog {
    public struct Voice: Hashable, Sendable, Identifiable {
        public let id: String
        public let note: String
        public var label: String { "\(id) · \(note)" }
    }

    public static let languages = DefaultVoices.supportedLanguages

    public static func languageName(_ code: String) -> String {
        Locale(identifier: "en").localizedString(forLanguageCode: code) ?? code
    }

    public static func voices(for language: String) -> [Voice] {
        all.sorted { lhs, rhs in
            if (lhs.id == "Kore") != (rhs.id == "Kore") { return lhs.id == "Kore" }
            return lhs.id < rhs.id
        }
    }

    public static func voice(id: String) -> Voice? { all.first { $0.id == id } }
    public static func defaultVoice(for language: String) -> String? { DefaultVoices.byLanguage[language] }

    // Google TTS prebuilt voice names and descriptors, checked 2026-09-29.
    // https://ai.google.dev/gemini-api/docs/speech-generation#prebuilt-voices
    // Table data © Google, CC BY 4.0: https://creativecommons.org/licenses/by/4.0/
    public static let all: [Voice] = [
        ("Zephyr", "Bright"), ("Puck", "Upbeat"), ("Charon", "Informative"),
        ("Kore", "Firm"), ("Fenrir", "Excitable"), ("Leda", "Youthful"),
        ("Orus", "Firm"), ("Aoede", "Breezy"), ("Callirrhoe", "Easy-going"),
        ("Autonoe", "Bright"), ("Enceladus", "Breathy"), ("Iapetus", "Clear"),
        ("Umbriel", "Easy-going"), ("Algieba", "Smooth"), ("Despina", "Smooth"),
        ("Erinome", "Clear"), ("Algenib", "Gravelly"), ("Rasalgethi", "Informative"),
        ("Laomedeia", "Upbeat"), ("Achernar", "Soft"), ("Alnilam", "Firm"),
        ("Schedar", "Even"), ("Gacrux", "Mature"), ("Pulcherrima", "Forward"),
        ("Achird", "Friendly"), ("Zubenelgenubi", "Casual"), ("Vindemiatrix", "Gentle"),
        ("Sadachbia", "Lively"), ("Sadaltager", "Knowledgeable"), ("Sulafat", "Warm"),
    ].map { Voice(id: $0.0, note: $0.1) }
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
