import Foundation
import PeekCore
import Testing

@testable import PeekUI

@Suite("Settings catalogs: Google voices and STT languages")
struct SettingsCatalogTests {
    @Test("all 30 Google voices work in every language preference")
    func voices() throws {
        #expect(VoiceCatalog.all.count == 30)
        #expect(Set(VoiceCatalog.all.map(\.id)).count == 30)
        #expect(VoiceCatalog.languages.contains("hi"))
        for language in VoiceCatalog.languages {
            #expect(VoiceCatalog.defaultVoice(for: language) == "Kore")
            #expect(VoiceCatalog.voices(for: language).first?.id == "Kore")
            #expect(!VoiceCatalog.languageName(language).isEmpty)
            for voice in VoiceCatalog.voices(for: language) {
                var settings = PeekSettings()
                try settings.apply(.voiceDefaults, .object([language: .string(voice.id)]))
                #expect(settings.voiceDefaults[language] == voice.id)
            }
        }
        #expect(VoiceCatalog.voice(id: "Kore")?.label == "Kore · Firm")
        #expect(VoiceCatalog.voice(id: "Puck")?.note == "Upbeat")
        #expect(VoiceCatalog.voice(id: "aura-2-thalia-en") == nil)
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
