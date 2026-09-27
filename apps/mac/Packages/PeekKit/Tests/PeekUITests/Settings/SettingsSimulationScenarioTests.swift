import Foundation
import PeekCore
import Testing

@testable import PeekUI

@Suite("Simulation scenarios and the --simulate launch argument")
struct SettingsSimulationScenarioTests {
    private let assets = SimulationAssetPaths(
        directory: URL(fileURLWithPath: "/tmp/peek-sim-assets"), cassette: URL(fileURLWithPath: "/tmp/peek-sim-assets/cassette.js"),
        cassetteSHA256: SimulationCassette.sha256)

    private func event(_ preset: SimulationPreset) throws -> PeekShowEvent {
        try preset.scenario.makeEvent(assets: assets, sendID: "snd_test", askID: "ask_test")
    }

    @Test("the presets are exactly the documented scenario names")
    func names() {
        #expect(SimulationPreset.allCases.map(\.rawValue) == [
            "show-text", "show-cover", "ask-single", "ask-multi", "ask-slider", "ask-range", "ask-text", "speak", "compact-show",
            "show-long", "show-long-mixed", "ask-long-labels", "ask-stepped", "compact-long",
            "queue-badge", "ask-compact", "ask-esc-hint",
        ])
        #expect(SimulationPreset.named(" Ask-Range ") == .askRange)
        #expect(SimulationPreset.named("ask") == nil)
        for preset in SimulationPreset.allCases { #expect(!preset.title.isEmpty) }
    }

    @Test("every preset is a valid peek.show event in the simulation context", arguments: SimulationPreset.allCases)
    func presetsAreValid(_ preset: SimulationPreset) throws {
        try preset.scenario.validate()
        let event = try event(preset)
        #expect(event.context == .simulation)
        #expect(event.sendID == "snd_test")
        #expect(event.slot == .top)
        try event.show?.validate()
        try event.ask?.validate()
        #expect(event.speak != nil || event.show != nil || event.ask != nil)
        #expect(event.askID == (event.ask == nil ? nil : "ask_test"), "only asks carry an ask id")
        if let speak = event.speak {
            #expect(speak.status == .pending, "Simulation always streams audio")
            #expect(!speak.text.isEmpty && speak.text.scalarCount <= SimulationScenario.maxSpeakScalars)
        }
        for path in (event.show?.imagePaths ?? []) + (event.ask?.imagePaths ?? []) {
            #expect(path.hasPrefix(assets.directory.path + "/"), "\(path)")
            #expect(path.hasSuffix(".png"))
        }
        // The event survives the wire format peekd would use.
        let decoded = try JSONDecoder().decode(PeekShowEvent.self, from: JSONEncoder().encode(event))
        #expect(decoded == event)
    }

    @Test("peek 0.1.2 presets: the queue badge (with its live update), the compact ask and the Esc hint")
    func presets012() throws {
        let queue = try event(.queueBadge)
        #expect(queue.queuedBehind == 3)
        #expect(queue.show != nil && queue.ask == nil)
        #expect(SimulationPreset.queueBadge.scenario.queueUpdate == 4)
        let compact = try event(.askCompact)
        #expect(compact.ask?.type == .singleChoice)
        #expect(compact.queuedBehind == 2)
        #expect(SimulationPreset.askCompact.scenario.askEsc == .collapsed)
        #expect(SimulationPreset.askEscHint.scenario.askEsc == .hint)
        #expect(try event(.askEscHint).queuedBehind == 0)
        for preset in SimulationPreset.allCases where ![.queueBadge, .askCompact, .askEscHint].contains(preset) {
            #expect(preset.scenario.askEsc == .none)
            #expect(preset.scenario.queuedBehind == 0)
        }
    }

    @Test("each preset shows what its name says")
    func presetContents() throws {
        let showText = try event(.showText)
        #expect(showText.speak == nil)
        #expect(showText.show?.elements == [.text(SimulationSamples.showText)])

        let cover = try event(.showCover)
        #expect(cover.speak?.text.contains("Neon Tide") == true)
        #expect(cover.show?.elements.count == ShowPayload.maxElements)
        #expect(cover.show?.imagePaths == [assets.cover(.neonTide)])
        if case .image(_, let caption)? = cover.show?.elements.first { #expect(caption == "Neon Tide") } else {
            Issue.record("the first show-cover element must be the cover image")
        }

        let single = try event(.askSingle)
        #expect(single.ask?.type == .singleChoice)
        #expect(single.ask?.options.map(\.id) == ["keep", "archive", "delete"])
        #expect(single.ask?.options.allSatisfy { $0.image != nil } == true, "ask-single has images in every option")

        let multi = try event(.askMulti)
        guard case .multipleChoice(let options, let min, let max)? = multi.ask?.kind else {
            Issue.record("ask-multi must be a multiple-choice ask")
            return
        }
        #expect(options.count == 4 && min == 1 && max == 4)
        #expect(options.allSatisfy { $0.image == nil })

        #expect(try event(.askSlider).ask?.type == .slider)
        #expect(try event(.askRange).ask?.type == .range)
        #expect(try event(.askText).ask?.type == .text)

        let speak = try event(.speak)
        #expect(speak.show == nil && speak.ask == nil && speak.speak?.text == SimulationSamples.speakOnly)

        #expect(SimulationPreset.compactShow.scenario.mode == .compact)
        #expect(try event(.compactShow).show?.elements == [.text(SimulationSamples.showText)])
        for preset in SimulationPreset.allCases where preset != .compactShow && preset != .compactLong {
            #expect(preset.scenario.mode == .normal)
        }

        // The ui-feedback.md scenarios.
        #expect(try event(.showLong).show?.elements == [.text(SimulationSamples.longText)])
        #expect(SimulationPreset.compactLong.scenario.mode == .compact)
        let mixed = try event(.showLongMixed)
        if case .image(_, let caption)? = mixed.show?.elements.first { #expect(caption == SimulationSamples.longCaption) }
        let labels = try event(.askLongLabels)
        #expect(labels.ask?.question == SimulationSamples.longQuestion)
        #expect(labels.ask?.options.allSatisfy { $0.label.count > 30 && $0.image == nil } == true)
        guard case .slider(let spec)? = try event(.askStepped).ask?.kind else {
            Issue.record("ask-stepped must be a slider")
            return
        }
        #expect(spec.min == 0 && spec.max == 10 && spec.step == 1)
    }

    @Test("every ask type validates with and without option images", arguments: AskType.allCases)
    func everyAskType(_ type: AskType) throws {
        for images in [true, false] {
            var scenario = SimulationScenario()
            scenario.content = .ask
            scenario.askType = type
            scenario.optionImages = images
            let event = try scenario.makeEvent(assets: assets, sendID: "snd_x", askID: "ask_x")
            #expect(event.ask?.type == type)
            #expect(event.durationMs == nil, "asks stay until answered")
            let hasImages = !(event.ask?.imagePaths.isEmpty ?? true)
            #expect(hasImages == (images && (type == .singleChoice || type == .multipleChoice)))
        }
    }

    @Test("every show variant validates, and holdMs becomes duration_ms", arguments: SimulationScenario.ShowVariant.allCases)
    func everyShowVariant(_ variant: SimulationScenario.ShowVariant) throws {
        var scenario = SimulationScenario()
        scenario.content = .show
        scenario.showVariant = variant
        scenario.holdMs = 90_000
        let event = try scenario.makeEvent(assets: assets, sendID: "snd_x", askID: "ask_x")
        #expect(event.durationMs == 90_000)
        #expect((1...3).contains(event.show?.elements.count ?? 0))
    }

    @Test("invalid scenarios explain what to change")
    func invalidScenarios() {
        var nothing = SimulationScenario()
        nothing.content = .none
        nothing.speak = false
        #expect(throws: SimulationScenarioError.nothingToShow) { try nothing.validate() }
        #expect(SimulationScenarioError.nothingToShow.description.contains("--speak, --show or --ask"))

        var empty = SimulationScenario()
        empty.speak = true
        empty.speakText = "  \n "
        #expect(throws: SimulationScenarioError.emptySpeech) { try empty.validate() }

        var long = SimulationScenario()
        long.speak = true
        long.speakText = String(repeating: "a", count: 2_001)
        #expect(throws: SimulationScenarioError.speechTooLong(2_001)) { try long.validate() }
    }

    @Test("sample speech follows the content")
    func sampleSpeech() {
        var scenario = SimulationScenario()
        scenario.content = .ask
        scenario.askType = .slider
        scenario.useSampleSpeech()
        #expect(scenario.speakText == "How loud should the music be?")
        scenario.content = .none
        scenario.useSampleSpeech()
        #expect(scenario.speakText == SimulationSamples.speakOnly)
    }

    @Test("ids have the D25 shape")
    func ids() {
        let id = SimulationScenario.newID("snd")
        #expect(id.hasPrefix("snd_"))
        #expect(id.count == 36)
        #expect(id.dropFirst(4).allSatisfy { $0.isHexDigit && !$0.isUppercase })
    }

    // MARK: --simulate

    @Test("--simulate is absent unless given")
    func launchAbsent() {
        #expect(SimulationLaunchHook.parse(["Peek"]) == nil)
        #expect(SimulationLaunchHook.parse(["Peek", "--after-update", "999", "--launched-by", "cli"]) == nil)
        #expect(SimulationLaunchHook.parse(["Peek", "--simulate-position", "3"]) == nil)
    }

    @Test("--simulate <name> and --simulate=<name> select a preset with defaults")
    func launchNames() throws {
        let spaced = try #require(SimulationLaunchHook.parse(["Peek", "--simulate", "ask-single"])).get()
        #expect(spaced == SimulationLaunchHook.Request(preset: .askSingle))
        #expect(spaced.holdSeconds == SimulationLaunchHook.defaultHoldSeconds)
        #expect(spaced.position == nil)
        let equals = try #require(SimulationLaunchHook.parse(["Peek", "-NSDocumentRevisionsDebugMode", "YES", "--simulate=compact-show"])).get()
        #expect(equals.preset == .compactShow)
        for preset in SimulationPreset.allCases {
            #expect(try SimulationLaunchHook.parse(["Peek", "--simulate", preset.rawValue])?.get().preset == preset)
        }
    }

    @Test("position and hold overrides apply to the scenario")
    func launchOverrides() throws {
        let request = try #require(
            SimulationLaunchHook.parse(["Peek", "--simulate-position=6", "--simulate", "show-cover", "--simulate-hold", "45"])
        ).get()
        #expect(request.position == .bottomLeft)
        #expect(request.holdSeconds == 45)
        let scenario = request.scenario
        #expect(scenario.position == .bottomLeft)
        #expect(scenario.holdMs == 45_000)
        #expect(scenario.showVariant == .threeElements)
        let event = try scenario.makeEvent(assets: assets, sendID: "snd_x", askID: "ask_x")
        #expect(event.slot == .bottomLeft)
        #expect(event.durationMs == 45_000)
    }

    @Test("bad launch arguments fail with a precise message")
    func launchErrors() {
        #expect(SimulationLaunchHook.parse(["Peek", "--simulate"]) == .failure(.missingScenario))
        #expect(SimulationLaunchHook.parse(["Peek", "--simulate", "--simulate-position", "2"]) == .failure(.missingScenario))
        #expect(SimulationLaunchHook.parse(["Peek", "--simulate", "dance"]) == .failure(.unknownScenario("dance")))
        #expect(SimulationLaunchHook.parse(["Peek", "--simulate", "speak", "--simulate-position", "9"]) == .failure(.invalidPosition("9")))
        #expect(SimulationLaunchHook.parse(["Peek", "--simulate", "speak", "--simulate-position"]) == .failure(.invalidPosition("")))
        #expect(SimulationLaunchHook.parse(["Peek", "--simulate", "speak", "--simulate-hold", "0"]) == .failure(.invalidHold("0")))
        #expect(SimulationLaunchHook.parse(["Peek", "--simulate", "speak", "--simulate-hold=soon"]) == .failure(.invalidHold("soon")))

        let unknown = SimulationLaunchHook.ParseError.unknownScenario("dance").description
        for preset in SimulationPreset.allCases { #expect(unknown.contains(preset.rawValue)) }
        #expect(SimulationLaunchHook.ParseError.invalidPosition("9").description.contains("1 (top centre) to 8 (top left)"))
    }
}
