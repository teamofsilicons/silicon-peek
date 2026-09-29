import Foundation
import Testing

@testable import PeekCore

@Suite("Models")
struct ModelsTests {
    private func decodeAsk(_ json: String) throws -> AskPayload {
        try JSONDecoder().decode(AskPayload.self, from: Data(json.utf8))
    }

    @Test("slot indices map to sides and Carbon key codes (1 top, clockwise)")
    func slots() {
        #expect(SlotIndex.allCases.map(\.side.rawValue)
            == ["top", "top-right", "right", "bottom-right", "bottom", "bottom-left", "left", "top-left"])
        #expect(SlotIndex.allCases.map(\.hotKeyCode) == [0x12, 0x13, 0x14, 0x15, 0x17, 0x16, 0x1A, 0x1C])
        #expect(SlotIndex(rawValue: 0) == nil)
        #expect(SlotIndex(rawValue: 9) == nil)
        #expect(SlotSide.bottom.index == .bottom)
        #expect(throws: DecodingError.self) { try JSONDecoder().decode(SlotIndex.self, from: Data("9".utf8)) }
    }

    @Test("hotkey modifiers produce Carbon masks and labels")
    func hotkeys() {
        #expect(HotkeyModifier.cmd.carbonMask == 256)
        #expect(HotkeyModifier.ctrlCmd.carbonMask == 256 | 4096)
        #expect(HotkeyModifier.optCmd.carbonMask == 256 | 2048)
        #expect(HotkeyModifier.cmd.label(for: .bottom) == "cmd+5")
        #expect(HotkeyModifier.ctrlCmd.symbols == "⌃⌘")
    }

    @Test("contexts round-trip through their wire strings")
    func contexts() throws {
        #expect(PeekContext(rawValue: "production") == .production)
        #expect(PeekContext(rawValue: "simulation") == .simulation)
        #expect(PeekContext(rawValue: "0192-uuid") == .testing(environmentID: "0192-uuid"))
        #expect(PeekContext.testing(environmentID: "x").inputContext == .testing)
        #expect(throws: DecodingError.self) { try JSONDecoder().decode(PeekContext.self, from: Data(#""""#.utf8)) }
    }

    @Test("ask option shorthand and missing ids default to 1, 2, …")
    func askOptionDefaults() throws {
        let ask = try decodeAsk(#"{"question":"Pick","type":"single_choice","options":["Red",{"label":"Blue"},{"id":"g","label":"Green"}]}"#)
        #expect(ask.options == [AskOption(id: "1", label: "Red"), AskOption(id: "2", label: "Blue"), AskOption(id: "g", label: "Green")])
        #expect(throws: Never.self) { try ask.validate() }
    }

    @Test("ask defaults: text max_length, multiple choice min/max, slider step/default, range default")
    func askDefaults() throws {
        #expect(try decodeAsk(#"{"question":"Why?","type":"text"}"#).kind == .text(placeholder: nil, maxLength: 500))
        #expect(try decodeAsk(#"{"question":"Which?","type":"multiple_choice","options":["a","b","c"]}"#).kind
            == .multipleChoice(options: [AskOption(id: "1", label: "a"), AskOption(id: "2", label: "b"), AskOption(id: "3", label: "c")],
                               min: 1, max: 3))
        #expect(try decodeAsk(#"{"question":"Volume?","type":"slider","min":0,"max":50}"#).kind
            == .slider(SliderSpec(min: 0, max: 50, step: 0.5, defaultValue: 0)))
        guard case .range(let spec) = try decodeAsk(#"{"question":"When?","type":"range","min":8,"max":20,"unit":"h"}"#).kind else {
            Issue.record("expected range")
            return
        }
        #expect(spec.defaultLower == 8)
        #expect(spec.defaultUpper == 20)
        #expect(spec.unit == "h")
        #expect(throws: DecodingError.self) {
            try decodeAsk(#"{"question":"When?","type":"range","min":8,"max":20,"default":[9]}"#)
        }
    }

    @Test("ask payloads round-trip through Codable")
    func askRoundTrip() throws {
        for json in [
            #"{"question":"Why?","type":"text","placeholder":"say it","max_length":80}"#,
            #"{"question":"Pick","type":"single_choice","options":[{"id":"a","label":"A","image":"/c/a.png"},"B"]}"#,
            #"{"question":"Which?","type":"multiple_choice","options":["a","b","c"],"min":0,"max":2}"#,
            #"{"question":"Volume?","type":"slider","min":0,"max":10,"step":1,"default":4,"unit":"dB"}"#,
            #"{"question":"When?","type":"range","min":8,"max":20,"default":[9,17]}"#,
        ] {
            let ask = try decodeAsk(json)
            let again = try JSONDecoder().decode(AskPayload.self, from: JSONEncoder().encode(ask))
            #expect(again == ask)
        }
    }

    @Test(
        "ask validation enforces §7.4",
        arguments: [
            (#"{"question":"","type":"text"}"#, "question_too_long"),
            (#"{"question":"Q","type":"single_choice","options":["only"]}"#, "too_many_options"),
            (#"{"question":"Q","type":"single_choice","options":["a","b","c","d","e","f","g"]}"#, "too_many_options"),
            (#"{"question":"Q","type":"single_choice","options":[{"id":"Bad Id","label":"a"},"b"]}"#, "invalid_input"),
            (#"{"question":"Q","type":"single_choice","options":[{"id":"x","label":"a"},{"id":"x","label":"b"}]}"#, "invalid_input"),
            (#"{"question":"Q","type":"multiple_choice","options":["a","b"],"min":2,"max":1}"#, "invalid_input"),
            (#"{"question":"Q","type":"slider","min":5,"max":5}"#, "invalid_input"),
            (#"{"question":"Q","type":"slider","min":0,"max":5,"default":9}"#, "invalid_input"),
            (#"{"question":"Q","type":"range","min":0,"max":5,"default":[4,2]}"#, "invalid_input"),
            (#"{"question":"Q","type":"text","max_length":0}"#, "invalid_input"),
        ])
    func askValidation(json: String, code: String) throws {
        let ask = try decodeAsk(json)
        #expect(throws: PayloadError.self) { try ask.validate() }
        do {
            try ask.validate()
        } catch {
            #expect(error.code == code)
        }
    }

    @Test("a question of exactly 80 scalars is accepted, 81 is not")
    func questionLimit() {
        let ok = AskPayload(question: String(repeating: "é", count: 80), kind: .text(placeholder: nil, maxLength: 500))
        #expect(throws: Never.self) { try ok.validate() }
        let long = AskPayload(question: String(repeating: "é", count: 81), kind: .text(placeholder: nil, maxLength: 500))
        #expect(throws: PayloadError.self) { try long.validate() }
    }

    @Test("show validation enforces element count and lengths")
    func showValidation() {
        #expect(throws: Never.self) { try ShowPayload(elements: [.text("hi"), .image(path: "/c/a.png", caption: nil)]).validate() }
        #expect(throws: PayloadError.self) { try ShowPayload(elements: []).validate() }
        #expect(throws: PayloadError.self) { try ShowPayload(elements: Array(repeating: .text("x"), count: 4)).validate() }
        #expect(throws: PayloadError.self) { try ShowPayload(elements: [.text(String(repeating: "x", count: 161))]).validate() }
        #expect(throws: PayloadError.self) {
            try ShowPayload(elements: [.image(path: "/a.png", caption: String(repeating: "c", count: 51))]).validate()
        }
    }

    @Test("ask values map to raw JSON and back by ask type")
    func askValues() {
        let cases: [(AskValue, AskType, JSONValue)] = [
            (.text("hello"), .text, "hello"),
            (.choice("keep"), .singleChoice, "keep"),
            (.choices(["a", "c"]), .multipleChoice, ["a", "c"]),
            (.number(3.5), .slider, 3.5),
            (.range(lower: 1, upper: 2), .range, [1.0, 2.0]),
        ]
        for (value, type, json) in cases {
            #expect(value.jsonValue == json)
            #expect(AskValue(json: json, for: type) == value)
        }
        #expect(AskValue(json: .int(4), for: .slider) == .number(4))
        #expect(AskValue(json: "x", for: .multipleChoice) == nil)
        #expect(AskValue.initial(for: AskPayload(question: "q", kind: .slider(SliderSpec(min: 0, max: 10, defaultValue: 3)))) == .number(3))
        #expect(AskValue.initial(for: AskPayload(question: "q", kind: .singleChoice(options: []))) == nil)
    }

    @Test("the default show duration follows clamp(3 + 0.06 × chars, 4, 15) s")
    func defaultDuration() {
        let short = PeekShowEvent(sendID: "s", slot: .top, show: ShowPayload(elements: [.text("hi")]))
        #expect(short.effectiveDuration == .seconds(4))
        let long = PeekShowEvent(sendID: "s", slot: .top, show: ShowPayload(elements: [.text(String(repeating: "x", count: 160))]))
        #expect(long.effectiveDuration == .milliseconds(12_600))
        let explicit = PeekShowEvent(sendID: "s", slot: .top, durationMs: 2000)
        #expect(explicit.effectiveDuration == .seconds(2))
    }

    @Test("moves are derived from successive slots.state tables")
    func slotMoves() {
        let old = [SlotState(index: .right, actorID: "si:dj", orgID: "tos"), SlotState(index: .top, actorID: "si:a", orgID: "tos")]
        let new = [SlotState(index: .bottom, actorID: "si:dj", orgID: "tos"), SlotState(index: .top, actorID: "si:a", orgID: "tos"),
                   SlotState(index: .left, actorID: "si:new", orgID: "tos")]
        #expect(SlotMove.between(old, new) == [SlotMove(key: SiliconKey(context: .production, orgID: "tos", actorID: "si:dj"),
                                                        from: .right, to: .bottom)])
    }

    @Test("Google voices accept custom IDs and reject malformed input")
    func voices() {
        for voice in ["Kore", "Puck", "voice_custom-123", "aura-2-thalia-en"] {
            #expect(DefaultVoices.isValidVoice(voice))
        }
        for voice in ["", "bad voice", "Kore/../../secret", "é", String(repeating: "a", count: 129)] {
            #expect(!DefaultVoices.isValidVoice(voice))
        }
        #expect(DefaultVoices.byLanguage.values.allSatisfy { $0 == "Kore" })
    }

    @Test("launch arguments: --after-update, --launched-by, AppKit pairs skipped")
    func launchArguments() {
        let parsed = AppLaunchArguments.parse(["/Applications/Peek.app/Contents/MacOS/Peek", "--after-update", "1000",
                                               "-NSDocumentRevisionsDebugMode", "YES", "--launched-by", "cli", "--weird"])
        #expect(parsed == AppLaunchArguments(afterUpdateFromBuild: 1000, launchedBy: "cli", unrecognized: ["--weird"]))
        #expect(AppLaunchArguments.parse(["peek", "--after-update=999"]).afterUpdateFromBuild == 999)
        #expect(AppLaunchArguments.parse(["peek", "--after-update"]).isAfterUpdate)
        #expect(!AppLaunchArguments.parse(["peek"]).isAfterUpdate)
        #expect(parsed.isLaunchedByCLI)
    }

    @Test("launch arguments: --uninstall and the Simulation flags are recognised, not reported as unknown")
    func launchArgumentsUninstallAndSimulation() {
        let parsed = AppLaunchArguments.parse([
            "Peek", "--uninstall", "--simulate", "ask-single", "--simulate-position=3", "--simulate-hold", "20",
            "--simulate-tone", "dark", "--simulate-mode", "compact", "--simulate-appearance", "light",
            "--simulate-backdrop", "/tmp/b.png",
        ])
        #expect(parsed.uninstall)
        #expect(parsed.unrecognized.isEmpty)
        #expect(parsed.wantsSimulation)
        #expect(parsed.simulation == [
            "--simulate", "ask-single", "--simulate-position=3", "--simulate-hold", "20", "--simulate-tone", "dark",
            "--simulate-mode", "compact", "--simulate-appearance", "light", "--simulate-backdrop", "/tmp/b.png",
        ])
        #expect(!AppLaunchArguments.parse(["Peek", "--launched-by", "cli"]).uninstall)
        #expect(!AppLaunchArguments.parse(["Peek"]).wantsSimulation)
    }

    @Test("paths: PEEK_SUPPORT_DIR and PEEK_CACHES_DIR replace the two Peek directories; empty means unset")
    func isolatedPaths() {
        let home = URL(fileURLWithPath: "/Users/someone", isDirectory: true)
        let plain = PeekPaths.fromEnvironment([:], home: home)
        #expect(plain == PeekPaths(home: home))
        #expect(!plain.isIsolated)
        #expect(plain.supportDirectory.path == "/Users/someone/Library/Application Support/Peek")
        #expect(plain.cachesDirectory.path == "/Users/someone/Library/Caches/Peek")

        let isolated = PeekPaths.fromEnvironment(
            ["PEEK_SUPPORT_DIR": "/tmp/peek-test/support", "PEEK_CACHES_DIR": "/tmp/peek-test/caches/"], home: home)
        #expect(isolated.isIsolated)
        #expect(isolated.supportDirectory.path == "/tmp/peek-test/support")
        #expect(isolated.settingsFile.path == "/tmp/peek-test/support/settings.json")
        #expect(isolated.uiLog.path == "/tmp/peek-test/support/ui.log")
        #expect(isolated.peekdLog.path == "/tmp/peek-test/support/peekd.log")
        #expect(isolated.imageCacheDirectory.path == "/tmp/peek-test/support/cache/images")
        #expect(isolated.cachesDirectory.path == "/tmp/peek-test/caches")
        #expect(isolated.installedApp.path == "/Users/someone/Applications/Peek.app")

        let empty = PeekPaths.fromEnvironment(["PEEK_SUPPORT_DIR": "", "PEEK_CACHES_DIR": ""], home: home)
        #expect(empty == plain)
    }

    @Test("runtime environment: PEEK_NO_SERVICES (and the older PEEK_SKIP_SERVICE_REGISTRATION), PEEK_API_URL")
    func runtimeEnvironment() {
        #expect(!PeekRuntimeEnvironment.fromEnvironment([:]).noServices)
        #expect(PeekRuntimeEnvironment.fromEnvironment(["PEEK_NO_SERVICES": "1"]).noServices)
        #expect(PeekRuntimeEnvironment.fromEnvironment(["PEEK_NO_SERVICES": "true"]).noServices)
        #expect(!PeekRuntimeEnvironment.fromEnvironment(["PEEK_NO_SERVICES": "0"]).noServices)
        #expect(PeekRuntimeEnvironment.fromEnvironment(["PEEK_SKIP_SERVICE_REGISTRATION": "1"]).noServices)
        let env = PeekRuntimeEnvironment.fromEnvironment([
            "PEEK_API_URL": "http://127.0.0.1:8080", "PEEK_DAEMON_SOCKET": "/tmp/x/peekd.sock",
        ])
        #expect(env.apiURL == "http://127.0.0.1:8080")
        #expect(env.daemonSocket == "/tmp/x/peekd.sock")
        #expect(PeekRuntimeEnvironment.fromEnvironment(["PEEK_API_URL": ""]).apiURL == nil)
    }

    @Test("paths are derived from the real home, never SILICON_HOME")
    func paths() {
        let paths = PeekPaths(home: URL(fileURLWithPath: "/Users/test"))
        #expect(paths.settingsFile.path == "/Users/test/Library/Application Support/Peek/settings.json")
        #expect(paths.cachesDirectory.path == "/Users/test/Library/Caches/Peek")
        #expect(paths.installedApp.path == "/Users/test/Applications/Peek.app")
        #expect(PeekPaths.realUserHome().path.hasPrefix("/"))
    }
}
