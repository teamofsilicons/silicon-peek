import Foundation
import PeekCore
import Testing

@testable import PeekUI

@Suite("Settings catalogs: Aura-2 voices and STT languages")
struct SettingsCatalogTests {
    @Test("the catalog has the 90 Aura-2 voices from notes/speech §1.4")
    func voiceCounts() {
        #expect(VoiceCatalog.all.count == 90)
        let counts = Dictionary(grouping: VoiceCatalog.all, by: \.language).mapValues(\.count)
        #expect(counts == ["en": 41, "es": 17, "nl": 9, "fr": 2, "de": 7, "it": 9, "ja": 5])
        #expect(Set(VoiceCatalog.all.map(\.id)).count == 90, "voice ids are unique")
        #expect(VoiceCatalog.languages == DefaultVoices.supportedLanguages)
    }

    @Test("every voice id is accepted by settings.json validation")
    func voiceIDsAreValid() throws {
        for voice in VoiceCatalog.all {
            #expect(DefaultVoices.isValidVoice(voice.id), "\(voice.id)")
            #expect(voice.id == "aura-2-\(voice.name.lowercased())-\(voice.language)")
            var settings = PeekSettings()
            try settings.apply(.voiceDefaults, .object([voice.language: .string(voice.id)]))
            #expect(settings.voiceDefaults[voice.language] == voice.id)
        }
    }

    @Test("each language's default voice is in the catalog, and featured voices come first")
    func defaultsAndOrder() throws {
        for language in VoiceCatalog.languages {
            let defaultID = try #require(VoiceCatalog.defaultVoice(for: language))
            #expect(VoiceCatalog.voice(id: defaultID) != nil, "\(defaultID)")
            let voices = VoiceCatalog.voices(for: language)
            let firstPlain = voices.firstIndex { !$0.featured } ?? voices.count
            #expect(voices[firstPlain...].allSatisfy { !$0.featured })
            #expect(!VoiceCatalog.languageName(language).isEmpty)
        }
        #expect(VoiceCatalog.voice(id: "aura-2-thalia-en")?.label == "Thalia · F · featured")
        #expect(VoiceCatalog.voice(id: "aura-2-selena-es")?.label == "Selena · F · Latin America · EN–ES code-switching")
        #expect(VoiceCatalog.voice(id: "aura-2-draco-en")?.note == "British")
    }

    @Test("every listed STT language is a valid stt_language value")
    func sttChoicesAreValid() throws {
        #expect(Set(STTLanguageChoices.common.map(\.tag)).count == STTLanguageChoices.common.count)
        for choice in STTLanguageChoices.common {
            #expect(PeekSettings.isLanguageTag(choice.tag), "\(choice.tag)")
            #expect(STTLanguageChoices.problem(with: choice.tag) == nil)
            var settings = PeekSettings()
            try settings.apply(.sttLanguage, .string(choice.tag))
        }
        #expect(STTLanguageChoices.problem(with: "auto") == nil)
        #expect(STTLanguageChoices.problem(with: "EN") != nil)
        #expect(STTLanguageChoices.problem(with: "english") != nil)
        #expect(STTLanguageChoices.automaticDescription(preferred: ["en-US", "hi-IN"]) == "en-US, hi-IN")
        #expect(STTLanguageChoices.automaticDescription(preferred: []) == "the system language")
    }
}
