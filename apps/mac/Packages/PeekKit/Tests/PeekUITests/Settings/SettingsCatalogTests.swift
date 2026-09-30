import Foundation
import PeekCore
import Testing

@testable import PeekUI

@Suite("Settings catalogs: ElevenLabs voices and STT languages")
struct SettingsCatalogTests {
    @Test("all 17 ElevenLabs voices work in every language preference")
    func voices() throws {
        #expect(VoiceCatalog.all.count == 17)
        #expect(Set(VoiceCatalog.all.map(\.id)).count == 17)
        #expect(VoiceCatalog.languages.contains("hi"))
        for language in VoiceCatalog.languages {
            #expect(VoiceCatalog.defaultVoice(for: language) == "JBFqnCBsd6RMkjVDRZzb")
            #expect(VoiceCatalog.voices(for: language).first?.id == "JBFqnCBsd6RMkjVDRZzb")
            #expect(!VoiceCatalog.languageName(language).isEmpty)
            for voice in VoiceCatalog.voices(for: language) {
                var settings = PeekSettings()
                try settings.apply(.voiceDefaults, .object([language: .string(voice.id)]))
                #expect(settings.voiceDefaults[language] == voice.id)
            }
        }
        #expect(VoiceCatalog.voice(id: "JBFqnCBsd6RMkjVDRZzb")?.label == "George")
        #expect(VoiceCatalog.voice(id: "DtsPFCrhbCbbJkwZsb3d")?.name == "Piper")
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
