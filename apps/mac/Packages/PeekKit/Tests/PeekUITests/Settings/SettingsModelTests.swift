import Foundation
import PeekCore
import Testing

@testable import PeekUI

@Suite("Settings model")
@MainActor
struct SettingsModelTests {
    private func makeModel(_ controls: SettingsSuiteControls = SettingsSuiteControls(),
                           login: SettingsSuiteLoginItems = SettingsSuiteLoginItems(),
                           screen: SettingsSuiteScreenCapture = SettingsSuiteScreenCapture()) -> SettingsModel {
        SettingsModel(controls: controls, loginItems: login, screenCapture: screen)
    }

    @Test("writes go through setSetting with the normalized value")
    func writesUseSetSetting() {
        let controls = SettingsSuiteControls()
        let model = makeModel(controls)
        model.setMode(.compact)
        model.setShowTestPeeks(false)
        model.resetAllVoices()  // already the defaults: nothing is sent
        #expect(controls.applied.map(\.0) == [.mode, .showTestPeeks])
        #expect(controls.applied.first?.1 == .string("compact"))
        #expect(controls.settings.mode == .compact)
    }

    @Test("speech-to-text: automatic, a listed tag, or a typed tag that must be BCP 47")
    func sttSelection() {
        let controls = SettingsSuiteControls()
        let model = makeModel(controls)
        #expect(model.sttSelection == .automatic)

        model.selectSTT(.tag("de"))
        #expect(controls.settings.sttLanguage == "de")
        #expect(model.sttSelection == .tag("de"))

        // "Other…" shows the text field at once, before any tag is typed.
        model.selectSTT(.custom)
        #expect(model.sttSelection == .custom)
        #expect(model.customSTTLanguage == "de", "the field starts from the current language")
        #expect(controls.settings.sttLanguage == "de")

        model.customSTTLanguage = "Portuguese"
        model.commitCustomSTTLanguage()
        #expect(model.customSTTProblem?.contains("not a BCP 47 tag") == true)
        #expect(controls.settings.sttLanguage == "de", "an invalid tag changes nothing")

        model.customSTTLanguage = "  gsw-CH "
        model.commitCustomSTTLanguage()
        #expect(model.customSTTProblem == nil)
        #expect(controls.settings.sttLanguage == "gsw-CH")
        #expect(model.sttSelection == .custom)
        #expect(model.customSTTLanguage == "gsw-CH")

        model.customSTTLanguage = ""
        model.commitCustomSTTLanguage()
        #expect(model.customSTTProblem?.contains("Type a BCP 47 language tag") == true)

        model.selectSTT(.automatic)
        #expect(controls.settings.sttLanguage == "auto")
        #expect(model.customSTTProblem == nil)
        #expect(model.sttSelection == .automatic)
    }

    @Test("a custom tag loaded from settings.json pre-fills the text field")
    func customTagIsPrefilled() {
        let controls = SettingsSuiteControls()
        controls.settings.sttLanguage = "sw"
        let model = makeModel(controls)
        #expect(model.sttSelection == .custom)
        #expect(model.customSTTLanguage == "sw")
    }

    @Test("the backdrop hint explains the permission state")
    func backdropHint() {
        let controls = SettingsSuiteControls()
        let screen = SettingsSuiteScreenCapture()
        let model = makeModel(controls, screen: screen)
        #expect(model.backdropHint?.contains("desktop picture") == true)

        model.setBackdrop(.screen)
        #expect(screen.requests == 1, "picking Screen asks for Screen Recording access right away")
        #expect(model.screenCaptureGranted == false)
        #expect(model.backdropHint?.contains("Screen Recording is not allowed") == true)

        screen.grantOnRequest = true
        model.requestScreenCapture()
        #expect(screen.requests == 2)
        #expect(model.screenCaptureGranted == true)
        #expect(model.backdropHint?.contains("twice a second") == true)
    }

    @Test("login item status is read from the provider and explained")
    func loginStatus() {
        let login = SettingsSuiteLoginItems()
        let model = makeModel(login: login)
        #expect(model.loginStatus == nil)
        model.refreshSystemStatus()
        let status = model.loginStatus
        #expect(status == LoginItemStatus(app: .enabled, helper: .requiresApproval))
        #expect(status?.needsApproval == true)
        #expect(status?.appSummary.hasPrefix("On.") == true)
        #expect(status?.helperSummary.contains("Login Items & Extensions") == true)
        model.loginItems.openLoginItemsSettings()
        #expect(login.openedSettings == 1)

        for state in [ServiceRegistrationState.notRegistered, .notFound, .unknown(9), .requiresApproval] {
            let text = LoginItemStatus(app: state, helper: state)
            #expect(!text.appSummary.isEmpty && !text.helperSummary.isEmpty)
            #expect(text.appSummary.hasSuffix(".") && text.helperSummary.hasSuffix("."))
        }
    }

    @Test("testing environments are grouped from the slot table")
    func testingEnvironments() {
        let controls = SettingsSuiteControls()
        let envA = "0f8e2c4a-1111-4a5b-9c3d-000000000001"
        let envB = "0f8e2c4a-2222-4a5b-9c3d-000000000002"
        controls.slots = [
            SlotState(index: .right, actorID: "si:dj", orgID: "tos"),
            SlotState(
                index: .right, context: .testing(environmentID: envA), actorID: "si:tester", orgID: "tos",
                environment: TestEnvironmentInfo(id: envA, name: "peek testing", generation: 3)),
            SlotState(
                index: .top, context: .testing(environmentID: envA), actorID: "si:cleanup", orgID: "tos",
                environment: TestEnvironmentInfo(id: envA, name: "peek testing", generation: 3)),
            SlotState(index: .left, context: .testing(environmentID: envB), actorID: "si:other", orgID: "acme"),
        ]
        let environments = makeModel(controls).testingEnvironments
        #expect(environments.map(\.id) == [envA, envB])
        #expect(environments[0].name == "peek testing")
        #expect(environments[0].pillText == "TEST · peek testing")
        #expect(environments[0].generation == 3)
        #expect(environments[0].occupants.map(\.slot) == [.top, .right], "occupants are sorted by position")
        #expect(environments[0].occupants.map(\.displayName) == ["cleanup", "tester"])
        #expect(environments[1].name == "Unnamed environment")
        #expect(environments[1].generation == nil)
    }

    @Test("the link state reads as a full sentence")
    func linkStateSentences() {
        #expect(DaemonLinkState.idle.statusSentence == "Not connected to peekd yet.")
        #expect(DaemonLinkState.connecting(attempt: 3).statusSentence == "Connecting to peekd (attempt 3)…")
        let hello = HelloResult(protocolVersion: 1, peekdVersion: "0.1.0")
        #expect(DaemonLinkState.connected(hello).statusSentence == "Connected to peekd 0.1.0, protocol 1.")
        #expect(DaemonLinkState.connected(hello).shortStatus == "peekd 0.1.0")
        let waiting = DaemonLinkState.waiting(retryIn: .milliseconds(1500), reason: "peekd is not running: no socket at /x")
        #expect(waiting.statusSentence == "peekd is not running: no socket at /x. Retrying in 1.5 s.")
        #expect(DaemonLinkState.waiting(retryIn: .seconds(12), reason: "Refused.").statusSentence == "Refused. Retrying in 12 s.")
    }

    @Test("the diagnostics report names versions, paths, settings and slots")
    func diagnosticsReport() {
        let controls = SettingsSuiteControls()
        controls.settingsWarnings = ["settings key \"mode\" must be one of \"normal\", \"compact\"; got \"x\""]
        controls.slots = [SlotState(index: .bottom, actorID: "si:dj", orgID: "tos")]
        controls.lastProblem = "cannot save settings: disk full"
        let diagnostics = DiagnosticsModel(
            paths: controls.paths, appInfo: SettingsAppInfo(version: "0.1.0", build: "1000", bundleIdentifier: "ai.tos.peek.dev"),
            socketPath: "/var/tmp/silicon-peek-501/peekd.sock")
        let report = diagnostics.report(controls: controls)
        #expect(report.contains("Peek.app 0.1.0 (1000) [ai.tos.peek.dev]"))
        #expect(report.contains("socket: /var/tmp/silicon-peek-501/peekd.sock"))
        #expect(report.contains("glass frosted"))
        #expect(report.contains("settings warning: settings key \"mode\""))
        #expect(report.contains("5 production si:dj[tos] drawing=none"))
        #expect(report.contains("last problem: cannot save settings: disk full"))
        #expect(report.contains("microphone: not asked yet"))
        #expect(SettingsAppInfo(version: nil, build: nil, bundleIdentifier: nil).versionText.hasPrefix("unknown"))
        #expect(diagnostics.appInfo.isDevelopmentBuild)
    }

    @Test("the diagnostics model reads the end of peekd.log off the main thread")
    func diagnosticsLog() async throws {
        let paths = settingsSuiteTemporaryPaths()
        defer { settingsSuiteRemove(paths) }
        let diagnostics = DiagnosticsModel(paths: paths, appInfo: SettingsAppInfo(bundle: .main), socketPath: "/tmp/x")
        await diagnostics.refreshLog()
        guard case .failed(.missing(let path)) = diagnostics.log else {
            Issue.record("expected a missing-file failure, got \(diagnostics.log)")
            return
        }
        #expect(path == paths.peekdLog.path)

        try AtomicFile.write(Data("peekd starting\npeekd listening on /var/tmp/x\n".utf8), to: paths.peekdLog)
        await diagnostics.refreshLog()
        guard case .loaded(let tail) = diagnostics.log else {
            Issue.record("expected the log to load, got \(diagnostics.log)")
            return
        }
        #expect(tail.lines == ["peekd starting", "peekd listening on /var/tmp/x"])
        #expect(diagnostics.loadedAt != nil)
    }
}
