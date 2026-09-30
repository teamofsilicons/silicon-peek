import Foundation
import PeekCore
import Testing

@testable import PeekUI

/// Settings persistence through the real ``PeekCoordinator``: SettingsModel → setSetting →
/// settings.json (atomic, 0600 in 0700) + `settings.changed` to peekd. The path is injected with
/// `PeekPaths(home:)` on a temporary directory, never the real ~/Library.
@Suite("Settings persistence")
@MainActor
struct SettingsPersistenceTests {
    private func makeCoordinator(paths: PeekPaths, link: SettingsSuiteLink) -> PeekCoordinator {
        let (settings, warnings) = PeekSettings.load(from: paths.settingsFile)
        let backdrop = SettingsSuiteBackdrop()
        return PeekCoordinator(
            link: link, drawing: SettingsSuiteDrawingRuntime(), speech: SettingsSuiteSpeech(), mic: SettingsSuiteMic(),
            images: SettingsSuiteImages(), input: SettingsSuiteHub(backdrop: backdrop), backdrop: backdrop, paths: paths,
            settings: settings, settingsWarnings: warnings, persistSettings: true)
    }

    private func makeModel(_ coordinator: PeekCoordinator) -> SettingsModel {
        SettingsModel(controls: coordinator, loginItems: SettingsSuiteLoginItems(), screenCapture: SettingsSuiteScreenCapture())
    }

    @Test("a missing settings.json gives the documented defaults and no warnings")
    func defaults() {
        let paths = settingsSuiteTemporaryPaths()
        defer { settingsSuiteRemove(paths) }
        let coordinator = makeCoordinator(paths: paths, link: SettingsSuiteLink())
        let model = makeModel(coordinator)

        #expect(coordinator.settingsWarnings.isEmpty)
        #expect(model.settings == PeekSettings.defaults)
        #expect(model.settings.mode == .normal)
        #expect(model.settings.hotkeyModifier == .ctrlCmd)
        #expect(model.settings.display == .main)
        #expect(model.settings.backdrop == .wallpaper)
        #expect(model.settings.telemetry)
        #expect(model.settings.showTestPeeks)
        #expect(model.settings.cliWatchdog)
        #expect(model.sttSelection == .automatic)
        #expect(!model.hasCustomVoices)
        for language in VoiceCatalog.languages {
            #expect(model.voice(for: language) == DefaultVoices.byLanguage[language])
        }
        #expect(!FileManager.default.fileExists(atPath: paths.settingsFile.path), "reading defaults must not create the file")
    }

    @Test("every setting round-trips through settings.json and is mirrored to peekd")
    func roundTrip() async throws {
        let paths = settingsSuiteTemporaryPaths()
        defer { settingsSuiteRemove(paths) }
        let link = SettingsSuiteLink()
        let coordinator = makeCoordinator(paths: paths, link: link)
        let model = makeModel(coordinator)

        model.setMode(.compact)
        model.setHotkeyModifier(.ctrlOptCmd)
        model.setDisplay(.pointer)
        model.setBackdrop(.screen)
        model.setTelemetry(false)
        model.setShowTestPeeks(false)
        model.setCLIWatchdog(false)
        model.setVoice("DtsPFCrhbCbbJkwZsb3d", for: "en")
        model.setVoice("Charon", for: "ja")
        model.selectSTT(.tag("pt-BR"))
        #expect(model.lastError == nil)

        let (reloaded, warnings) = PeekSettings.load(from: paths.settingsFile)
        #expect(warnings.isEmpty)
        #expect(reloaded == coordinator.settings)
        #expect(reloaded.mode == .compact)
        #expect(reloaded.hotkeyModifier == .ctrlOptCmd)
        #expect(reloaded.display == .pointer)
        #expect(reloaded.backdrop == .screen)
        #expect(!reloaded.telemetry)
        #expect(!reloaded.showTestPeeks)
        #expect(!reloaded.cliWatchdog)
        #expect(reloaded.voiceDefaults["en"] == "DtsPFCrhbCbbJkwZsb3d")
        #expect(reloaded.voiceDefaults["ja"] == "Charon")
        #expect(reloaded.voiceDefaults["de"] == DefaultVoices.byLanguage["de"])
        #expect(reloaded.sttLanguage == "pt-BR")

        // settings.json is private to the user.
        let fileMode = try #require(
            try FileManager.default.attributesOfItem(atPath: paths.settingsFile.path)[.posixPermissions] as? NSNumber)
        #expect(fileMode.intValue & 0o777 == 0o600)
        let directoryMode = try #require(
            try FileManager.default.attributesOfItem(atPath: paths.supportDirectory.path)[.posixPermissions] as? NSNumber)
        #expect(directoryMode.intValue & 0o777 == 0o700)

        // Each change went to peekd as settings.changed with the normalized value.
        let mirrored = await settingsSuiteWait { await link.settingsChanges().count == 10 }
        #expect(mirrored)
        // Each request is sent from its own task, so compare as a multiset rather than by order.
        let changes = await link.settingsChanges()
        #expect(changes.map(\.key).sorted() == [
            "backdrop", "display", "hotkey_modifier", "mode", "show_test_peeks", "stt_language", "telemetry",
            "updates.cli_watchdog", "voice_defaults", "voice_defaults",
        ])
        #expect(changes.contains { $0.key == "mode" && $0.value == .string("compact") })
        #expect(changes.contains { $0.key == "stt_language" && $0.value == .string("pt-BR") })
        #expect(changes.contains { $0.key == "telemetry" && $0.value == .bool(false) })
        let voiceTables = changes.filter { $0.key == "voice_defaults" }.compactMap(\.value.objectValue)
        #expect(voiceTables.allSatisfy { $0.count == DefaultVoices.byLanguage.count }, "voice_defaults is always the full table")
        #expect(voiceTables.contains { $0["ja"] == .string("Charon") && $0["en"] == .string("DtsPFCrhbCbbJkwZsb3d") })
    }

    @Test("keys this build does not know survive a change")
    func unknownKeysArePreserved() throws {
        let paths = settingsSuiteTemporaryPaths()
        defer { settingsSuiteRemove(paths) }
        let original = #"{"schema":1,"future_key":{"nested":[1,2]},"mode":"normal","updates":{"cli_watchdog":true,"channel":"beta"}}"#
        try AtomicFile.write(Data(original.utf8), to: paths.settingsFile)
        let coordinator = makeCoordinator(paths: paths, link: SettingsSuiteLink())
        #expect(coordinator.settingsWarnings.isEmpty)

        makeModel(coordinator).setMode(.compact)

        let written = try StrictJSON.parse(Data(contentsOf: paths.settingsFile))
        #expect(written["mode"] == .string("compact"))
        #expect(written["future_key"] == .object(["nested": .array([.int(1), .int(2)])]))
        #expect(written["updates"]?["channel"] == .string("beta"))
        #expect(written["updates"]?["cli_watchdog"] == .bool(true))
        #expect(written["schema"] == .int(1))
    }

    @Test("an invalid value is refused with the reason and never written or mirrored")
    func invalidValuesAreRefused() async {
        let paths = settingsSuiteTemporaryPaths()
        defer { settingsSuiteRemove(paths) }
        let link = SettingsSuiteLink()
        let coordinator = makeCoordinator(paths: paths, link: link)
        let model = makeModel(coordinator)

        #expect(!model.apply(.sttLanguage, .string("English")))
        #expect(model.lastError?.contains("BCP 47") == true)
        #expect(!model.apply(.voiceDefaults, .object(["en": .string("bad voice")])))
        #expect(model.lastError?.contains("ElevenLabs voice") == true)
        #expect(!model.apply(.voiceDefaults, .object(["english": .string("JBFqnCBsd6RMkjVDRZzb")])))
        #expect(model.lastError?.contains("primary language code") == true)
        #expect(!model.apply(.mode, .string("tiny")))
        #expect(model.lastError?.contains("\"normal\", \"compact\"") == true)

        #expect(coordinator.settings == PeekSettings.defaults)
        #expect(!FileManager.default.fileExists(atPath: paths.settingsFile.path))
        try? await Task.sleep(for: .milliseconds(50))
        #expect(await link.settingsChanges().isEmpty)

        // The next accepted change clears the error.
        #expect(model.apply(.mode, .string("compact")))
        #expect(model.lastError == nil)
    }

    @Test("setting a value that is already in effect writes and sends nothing")
    func unchangedValuesAreNotResent() async {
        let paths = settingsSuiteTemporaryPaths()
        defer { settingsSuiteRemove(paths) }
        let link = SettingsSuiteLink()
        let model = makeModel(makeCoordinator(paths: paths, link: link))

        model.setMode(.normal)
        model.resetAllVoices()
        model.selectSTT(.automatic)

        #expect(!FileManager.default.fileExists(atPath: paths.settingsFile.path))
        try? await Task.sleep(for: .milliseconds(50))
        #expect(await link.settingsChanges().isEmpty)
    }

    @Test("resetting voices restores the defaults and sends the full table")
    func resetVoices() async throws {
        let paths = settingsSuiteTemporaryPaths()
        defer { settingsSuiteRemove(paths) }
        let link = SettingsSuiteLink()
        let coordinator = makeCoordinator(paths: paths, link: link)
        let model = makeModel(coordinator)

        model.setVoice("Orus", for: "fr")
        #expect(model.isVoiceCustomized(for: "fr"))
        model.resetVoice(for: "fr")
        #expect(!model.isVoiceCustomized(for: "fr"))
        model.setVoice("Aoede", for: "nl")
        model.resetAllVoices()
        #expect(!model.hasCustomVoices)
        #expect(PeekSettings.load(from: paths.settingsFile).settings.voiceDefaults == DefaultVoices.byLanguage)

        let sent = await settingsSuiteWait { await link.settingsChanges().count == 4 }
        #expect(sent)
        let changes = await link.settingsChanges()
        #expect(changes.allSatisfy { $0.key == "voice_defaults" })
        // fr reset and "Reset All" both send the default table.
        #expect(changes.filter { $0.value == PeekSettings.defaults.value(for: .voiceDefaults) }.count == 2)
    }
}
