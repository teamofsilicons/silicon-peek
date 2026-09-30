import Foundation
import Testing

@testable import PeekCore

@Suite("Settings")
struct SettingsTests {
    private func temporaryDirectory() throws -> URL {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("peek-settings-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }

    @Test("defaults match §1.7")
    func defaults() {
        let settings = PeekSettings.defaults
        #expect(settings.mode == .normal)
        #expect(settings.hotkeyModifier == .ctrlCmd)
        #expect(settings.display == .main)
        #expect(settings.backdrop == .wallpaper)
        #expect(settings.telemetry)
        #expect(settings.showTestPeeks)
        #expect(settings.voiceDefaults["en"] == "JBFqnCBsd6RMkjVDRZzb")
        #expect(settings.sttLanguage == "auto")
        #expect(settings.cliWatchdog)
    }

    @Test("the document shape nests updates.cli_watchdog and carries schema")
    func documentShape() {
        let json = PeekSettings.defaults.jsonValue
        #expect(json["schema"] == 1)
        #expect(json["updates"] == ["cli_watchdog": true])
        #expect(json["hotkey_modifier"] == "ctrl+cmd", "the default shared with peekd (DEFAULT_HOTKEY_MODIFIER)")
        #expect(json["updates.cli_watchdog"] == nil)
    }

    @Test("decoding is lenient per key and keeps unknown keys")
    func lenientDecode() {
        let data = Data(#"""
            {"schema":1,"mode":"compact","hotkey_modifier":"hyper","telemetry":"yes","stt_language":"pt-BR",
             "voice_defaults":{"es":"DtsPFCrhbCbbJkwZsb3d"},"updates":{"cli_watchdog":false,"channel":"beta"},"future":{"x":1}}
            """#.utf8)
        let (settings, warnings) = PeekSettings.decode(data)
        #expect(settings.mode == .compact)
        #expect(settings.hotkeyModifier == .ctrlCmd)
        #expect(settings.telemetry == true)
        #expect(settings.sttLanguage == "pt-BR")
        #expect(settings.voiceDefaults["es"] == "DtsPFCrhbCbbJkwZsb3d")
        #expect(settings.voiceDefaults["en"] == "JBFqnCBsd6RMkjVDRZzb")
        #expect(settings.cliWatchdog == false)
        #expect(settings.extra["future"] == ["x": 1])
        #expect(settings.jsonValue["updates"] == ["cli_watchdog": false, "channel": "beta"])
        #expect(warnings.count == 2)
        #expect(warnings.contains { $0.contains("hotkey_modifier") && $0.contains("\"ctrl+cmd\"") })
        #expect(warnings.contains { $0.contains("telemetry") })
    }

    @Test("invalid JSON or a non-object falls back to defaults with a warning")
    func invalidDocuments() {
        #expect(PeekSettings.decode(Data("{".utf8)).settings == PeekSettings.defaults)
        #expect(PeekSettings.decode(Data("{".utf8)).warnings.count == 1)
        #expect(PeekSettings.decode(Data("[]".utf8)).warnings.first?.contains("object") == true)
        #expect(PeekSettings.decode(Data(#"{"mode":"normal","mode":"compact"}"#.utf8)).warnings.first?.contains("duplicate") == true)
    }

    @Test("apply validates values and null resets to the default")
    func apply() throws {
        var settings = PeekSettings()
        try settings.apply(.display, "pointer")
        #expect(settings.display == .pointer)
        try settings.apply(.display, .null)
        #expect(settings.display == .main)
        #expect(throws: SettingsError.self) { try settings.apply(.backdrop, "camera") }
        #expect(throws: SettingsError.self) { try settings.apply(.sttLanguage, "English") }
        #expect(throws: SettingsError.self) { try settings.apply(.voiceDefaults, ["english": "JBFqnCBsd6RMkjVDRZzb"]) }
        #expect(throws: SettingsError.self) { try settings.apply(.voiceDefaults, ["en": "bad voice"]) }
        try settings.apply(.voiceDefaults, ["hi": "DtsPFCrhbCbbJkwZsb3d", "ta": "voice_tamil", "en": "JBFqnCBsd6RMkjVDRZzb"])
        #expect(settings.voiceDefaults["hi"] == "DtsPFCrhbCbbJkwZsb3d")
        #expect(settings.voiceDefaults["ta"] == "voice_tamil")
        #expect(settings.voiceDefaults["en"] == "JBFqnCBsd6RMkjVDRZzb")
        try settings.apply(.cliWatchdog, false)
        #expect(settings.value(for: .cliWatchdog) == false)
    }

    @Test("write + load round-trips atomically with mode 0600 in a 0700 directory")
    func writeAndLoad() throws {
        let root = try temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let paths = PeekPaths(home: root)
        var settings = PeekSettings()
        settings.mode = .compact
        settings.showTestPeeks = false
        settings.extra["future"] = "kept"
        try settings.write(to: paths.settingsFile)

        let (loaded, warnings) = PeekSettings.load(from: paths.settingsFile)
        #expect(warnings.isEmpty)
        #expect(loaded == settings)

        let attributes = try FileManager.default.attributesOfItem(atPath: paths.settingsFile.path)
        #expect((attributes[.posixPermissions] as? NSNumber)?.intValue == 0o600)
        let dirAttributes = try FileManager.default.attributesOfItem(atPath: paths.supportDirectory.path)
        #expect((dirAttributes[.posixPermissions] as? NSNumber)?.intValue == 0o700)
        let leftovers = try FileManager.default.contentsOfDirectory(atPath: paths.supportDirectory.path).filter { $0.hasSuffix(".tmp") }
        #expect(leftovers.isEmpty)
    }

    @Test("a missing settings file silently yields defaults")
    func missingFile() throws {
        let root = try temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }
        let (settings, warnings) = PeekSettings.load(from: root.appendingPathComponent("nope.json"))
        #expect(settings == PeekSettings.defaults)
        #expect(warnings.isEmpty)
    }

    @Test("BCP 47 checks accept common tags")
    func languageTags() {
        #expect(PeekSettings.isLanguageTag("en"))
        #expect(PeekSettings.isLanguageTag("pt-BR"))
        #expect(PeekSettings.isLanguageTag("zh-Hant-TW"))
        #expect(!PeekSettings.isLanguageTag("EN"))
        #expect(!PeekSettings.isLanguageTag("e"))
        #expect(!PeekSettings.isLanguageTag("en-"))
    }
}
