import Foundation
import PeekCore
import Testing

@testable import PeekUI

/// The Simulation engine end to end against a presenter that behaves like the coordinator at its
/// seams (``SettingsSuitePresenter``): slot table, `drawing.load` through the link, `peek.show`,
/// streamed TTS, and what the bubble sends back. Nothing here opens a socket.
@Suite("Simulation engine")
@MainActor
struct SettingsSimulationEngineTests {
    private func presenter(_ factory: SettingsSuitePipelineFactory) throws -> SettingsSuitePresenter {
        try #require(factory.presenters.last, "the engine never built its presenter")
    }

    @Test("simulate presents a real peek.show with the cassette loaded through drawing.load")
    func presents() async throws {
        let factory = SettingsSuitePipelineFactory()
        let engine = SimulationEngine(dependencies: factory.dependencies)
        var scenario = SimulationPreset.askSingle.scenario
        scenario.position = .right

        #expect(await engine.simulate(scenario))
        let presenter = try presenter(factory)
        #expect(presenter.started == 1)

        // One slot, in the simulation context, with no hotkey and the cassette as its drawing.
        let table = try #require(presenter.slotTables.last)
        #expect(table.count == 1)
        #expect(table[0].index == .right)
        #expect(table[0].context == .simulation)
        #expect(table[0].siliconKey == SimulationEngine.siliconKey)
        #expect(!table[0].hotkey, "Simulation never registers a Carbon hotkey")
        #expect(table[0].drawing?.sha256 == SimulationCassette.sha256)

        // drawing.load went through the presenter's own handler into the (wrapped) drawing runtime.
        let load = try #require(presenter.loads.first)
        #expect(load.siliconKey == SimulationEngine.siliconKey)
        #expect(load.sha256 == SimulationCassette.sha256)
        #expect(FileManager.default.contents(atPath: load.scriptPath) == SimulationCassette.data)
        let host = try #require(factory.drawing.madeHosts.first)
        #expect(host.loaded?.sha256 == SimulationCassette.sha256)
        #expect(engine.drawingStatus == .ready(sha256: SimulationCassette.sha256))
        #expect(engine.log.contains { $0.kind == .drawing && $0.text.hasPrefix("loaded cassette.js") }, "peek.log reaches the window")

        // The event is the scenario's ask, with images that exist on disk.
        let event = try #require(presenter.presented.last)
        #expect(event.slot == .right)
        #expect(event.context == .simulation)
        #expect(event.ask?.type == .singleChoice)
        #expect(event.askID?.hasPrefix("ask_") == true)
        for path in event.ask?.imagePaths ?? [] { #expect(FileManager.default.fileExists(atPath: path)) }
        #expect(engine.status == .presenting(sendID: event.sendID, slot: .right))
    }

    @Test("speech streams as tts.begin, ordered chunks and tts.end to the presenter's speech player")
    func streamsSpeech() async throws {
        let factory = SettingsSuitePipelineFactory()
        let engine = SimulationEngine(dependencies: factory.dependencies)
        #expect(await engine.simulate(SimulationPreset.speak.scenario))
        let presenter = try presenter(factory)
        let sendID = try #require(presenter.presented.last?.sendID)

        let ended = await settingsSuiteWait {
            presenter.events.contains { if case .ttsEnd = $0 { true } else { false } }
        }
        #expect(ended)
        guard case .ttsBegin(let begin)? = presenter.events.first else {
            Issue.record("the first event must be tts.begin, got \(String(describing: presenter.events.first))")
            return
        }
        #expect(begin.sendID == sendID)
        #expect(begin.format == "s16le" && begin.sampleRate == 24_000 && begin.channels == 1)
        let chunks = presenter.events.compactMap { event -> TTSChunk? in
            if case .ttsChunk(let chunk) = event { return chunk }
            return nil
        }
        #expect(!chunks.isEmpty)
        #expect(chunks.map(\.seq) == Array(0..<chunks.count))
        #expect(chunks.allSatisfy { $0.sendID == sendID && $0.pcm.count <= FrameLimits.maxTTSChunkBytes && $0.pcm.count % 2 == 0 })
        let totalBytes = chunks.reduce(0) { $0 + $1.pcm.count }
        guard case .ttsEnd(let end)? = presenter.events.last(where: { if case .ttsEnd = $0 { true } else { false } }) else {
            Issue.record("no tts.end")
            return
        }
        #expect(end.totalFrames * 2 == totalBytes)
        #expect(end.totalFrames == SimulationSpeechSynth.synthesize(SimulationSamples.speakOnly).frames)

        // The engine reads playback back from the speech player for the window's meter.
        let polled = await settingsSuiteWait { engine.playback?.done == true }
        #expect(polled)
        #expect(factory.speeches.last?.events.count == chunks.count + 2)
    }

    @Test("appearance, context and backdrop tone reach the drawing's input")
    func overrides() async throws {
        let factory = SettingsSuitePipelineFactory()
        let engine = SimulationEngine(dependencies: factory.dependencies)
        var scenario = SimulationPreset.showText.scenario
        scenario.appearance = .dark
        scenario.context = .testing
        scenario.backdropTone = .dark
        #expect(await engine.simulate(scenario))

        let presenter = try presenter(factory)
        let source = presenter.parts.input.source(for: SimulationEngine.siliconKey)
        let snapshot = source.snapshot(t: 1, dt: 0.016)
        #expect(snapshot.appearance == .dark)
        #expect(snapshot.context == .testing)
        #expect(snapshot.backdrop.tone == .dark)
        #expect(snapshot.backdrop.ink == "#ffffff")
        #expect(presenter.parts.input.appearance == .dark, "the chrome sees the simulated appearance too")

        scenario.appearance = .system
        scenario.context = .simulation
        scenario.backdropTone = .light
        #expect(await engine.simulate(scenario))
        let next = source.snapshot(t: 2, dt: 0.016)
        #expect(next.appearance == .light, "system follows the wrapped hub")
        #expect(next.context == .simulation)
        #expect(next.backdrop.tone == .light)
    }

    @Test("the presenter starts in the scenario's mode and later changes arrive as settings, never persisted")
    func mode() async throws {
        let factory = SettingsSuitePipelineFactory()
        let engine = SimulationEngine(dependencies: factory.dependencies)
        #expect(await engine.simulate(SimulationPreset.compactShow.scenario))
        let presenter = try presenter(factory)
        #expect(presenter.parts.settings.mode == .compact)
        #expect(presenter.settingsChanges.isEmpty)

        #expect(await engine.simulate(SimulationPreset.showText.scenario))
        #expect(presenter.settingsChanges.map(\.0) == [.mode])
        #expect(presenter.settingsChanges.first?.1 == .string("normal"))
        #expect(factory.presenters.count == 1, "one presenter serves every run")
        #expect(!FileManager.default.fileExists(atPath: factory.paths.settingsFile.path))
    }

    @Test("a new simulation replaces the bubble on screen; stop slides it out and silences it")
    func replaceAndStop() async throws {
        let factory = SettingsSuitePipelineFactory()
        let engine = SimulationEngine(dependencies: factory.dependencies)
        #expect(await engine.simulate(SimulationPreset.showCover.scenario))
        let presenter = try presenter(factory)
        let first = try #require(presenter.presented.last?.sendID)

        #expect(await engine.simulate(SimulationPreset.askText.scenario))
        let second = try #require(presenter.presented.last?.sendID)
        #expect(first != second)
        #expect(presenter.cancelled.first?.0 == first)
        #expect(presenter.cancelled.first?.1 == .other("simulation_replaced"))
        #expect(presenter.loads.count == 1, "the cassette is loaded once")

        engine.stop()
        #expect(presenter.cancelled.last?.0 == second)
        #expect(presenter.cancelled.last?.1 == .other("simulation_stopped"))
        #expect(factory.speeches.last?.stopped.contains(second) == true)
        #expect(engine.status == .idle)
    }

    @Test("what the bubble sends is logged, closes the bubble, and voice gets a failed stt.result")
    func requestsFromTheBubble() async throws {
        let factory = SettingsSuitePipelineFactory()
        let engine = SimulationEngine(dependencies: factory.dependencies)
        #expect(await engine.simulate(SimulationPreset.askSingle.scenario))
        let presenter = try presenter(factory)
        let event = try #require(presenter.presented.last)
        let askID = try #require(event.askID)

        try await presenter.send(
            VoiceSubmitRequest(sendID: event.sendID, askID: askID, slot: event.slot, durationMs: 2_400), blobs: [Data(count: 4_000)])
        let transcribed = await settingsSuiteWait {
            presenter.events.contains { if case .sttResult = $0 { true } else { false } }
        }
        #expect(transcribed)
        let result = presenter.events.compactMap { event -> STTResultEvent? in
            if case .sttResult(let result) = event { return result }
            return nil
        }.first
        #expect(result?.askID == askID)
        #expect(result?.outcome == .failed)
        #expect(result?.error?.code == SimulationLink.noTranscriptionCode)

        try await presenter.send(AnswerRequest(sendID: event.sendID, askID: askID, value: .choice("keep"), via: .click))
        let closed = await settingsSuiteWait { if case .closed = engine.status { true } else { false } }
        #expect(closed)
        #expect(engine.status == .closed("answered \"keep\" via click"))
        #expect(engine.log.contains { $0.kind == .request && $0.text.hasPrefix("voice.submit: 2.4 s WAV") })
        #expect(engine.log.contains { $0.kind == .request && $0.text.hasPrefix("answer \"keep\" via click") })
    }

    @Test("an invalid scenario never builds a presenter")
    func invalidScenario() async {
        let factory = SettingsSuitePipelineFactory()
        let engine = SimulationEngine(dependencies: factory.dependencies)
        var scenario = SimulationScenario()
        scenario.content = .none
        scenario.speak = false
        engine.scenario = scenario
        #expect(engine.validationError?.contains("Nothing to simulate") == true)
        #expect(!(await engine.simulate()))
        #expect(factory.presenters.isEmpty)
        #expect(engine.status == .failed(SimulationScenarioError.nothingToShow.description))
        #expect(engine.log.last?.kind == .error)
    }

    @Test("shutdown stops the presenter and the link and unloads the drawing")
    func shutdown() async throws {
        let factory = SettingsSuitePipelineFactory()
        let engine = SimulationEngine(dependencies: factory.dependencies)
        #expect(await engine.simulate(SimulationPreset.showText.scenario))
        let presenter = try presenter(factory)
        #expect(engine.isRunning)

        await engine.shutdown()
        #expect(!engine.isRunning)
        #expect(presenter.stopped == 1)
        #expect(presenter.dismissAllCount == 1)
        #expect(await presenter.parts.link.currentState == .stopped)
        #expect(engine.status == .idle)

        // The next run builds a fresh presenter and loads the drawing again.
        #expect(await engine.simulate(SimulationPreset.showText.scenario))
        #expect(factory.presenters.count == 2)
        #expect(factory.presenters.last?.loads.count == 1)
    }

    @Test("the log is capped and copyable")
    func logCap() {
        let engine = SimulationEngine(dependencies: SettingsSuitePipelineFactory().dependencies)
        for index in 0..<(SimulationEngine.maxLogEntries + 20) { engine.append(.info, "line \(index)") }
        #expect(engine.log.count == SimulationEngine.maxLogEntries)
        #expect(engine.log.first?.text == "line 20")
        #expect(engine.logText.components(separatedBy: "\n").count == SimulationEngine.maxLogEntries)
        #expect(engine.logText.contains("[info] line 519"))
        engine.clearLog()
        #expect(engine.log.isEmpty)
    }
}

@Suite("Simulation link")
struct SettingsSimulationLinkTests {
    @Test("requests need a started link and respect the frame limits")
    func sendRules() async throws {
        let link = SimulationLink()
        await #expect(throws: DaemonLinkError.self) {
            _ = try await link.send(FocusRequest(slot: .top), blobs: [], timeout: nil)
        }
        await link.start()
        let oversized = Data(count: FrameLimits.maxWAVBytes + 1)
        do {
            _ = try await link.send(
                VoiceSubmitRequest(sendID: nil, askID: nil, slot: .top, durationMs: 1), blobs: [oversized], timeout: nil)
            Issue.record("a WAV over 4 MiB must be refused")
        } catch {
            guard case .invalidRequest(let op, let reason) = error else {
                Issue.record("unexpected error \(error)")
                return
            }
            #expect(op == "voice.submit")
            #expect(reason.contains("\(FrameLimits.maxWAVBytes)"))
        }
        let ack = try await link.send(FocusRequest(slot: .left), blobs: [], timeout: nil)
        #expect(ack.result["simulated"] == .bool(true))
    }

    @Test("states start with the current state, and stop finishes every stream")
    func streams() async {
        let link = SimulationLink()
        var states = await link.states().makeAsyncIterator()
        #expect(await states.next() == .idle)
        await link.start()
        let connected = await states.next()
        #expect(connected?.isConnected == true)
        let events = await link.events()
        await link.push(.peekCancel(PeekCancelEvent(sendID: "snd_x", reason: .expired)))
        await link.stop()
        var received: [DaemonEvent] = []
        for await event in events { received.append(event) }
        #expect(received == [.peekCancel(PeekCancelEvent(sendID: "snd_x", reason: .expired))])
        #expect(await states.next() == .stopped)
        #expect(await states.next() == nil)
    }

    @Test("a peekd→UI request without a handler is answered unknown_op")
    func requestWithoutHandler() async {
        let link = SimulationLink()
        let reply = await link.request(.appQuit(AppBuildRequest(build: 1)))
        guard case .failure(let error) = reply else {
            Issue.record("expected a failure, got \(reply)")
            return
        }
        #expect(error.code == "unknown_op")
        #expect(!(await link.hasRequestHandler))
    }

    @Test("request records describe what peekd and the Silicon would have done")
    func describer() {
        func describe(_ op: String, _ fields: [String: JSONValue], blobs: [Int] = []) -> SimulationRequestDescriber.Description {
            SimulationRequestDescriber.describe(SimulationRequestRecord(op: op, fields: .object(fields), blobSizes: blobs))
        }
        let dismissed = describe("dismissed", ["send_id": "snd_1", "gesture": "down_arrow_double"])
        #expect(dismissed.closesBubble == "dismissed (down_arrow_double)")
        let shown = describe("shown.done", ["send_id": "snd_1", "visible_ms": 6_100, "reason": "auto"])
        #expect(shown.text == "shown.done after 6.1 s (auto)")
        #expect(shown.closesBubble != nil)
        let speech = describe("speech.done", ["played_ms": 1_200, "total_ms": 4_000, "stopped_by_user": true])
        #expect(speech.text == "speech.done: played 1.2 s of 4.0 s, stopped by a double click")
        #expect(speech.closesBubble == nil)
        #expect(describe("drawing.error", ["reason": "oom", "message": "out of memory"]).isError)
        #expect(describe("message", ["text": "hello"]).text.contains("peek.message.received"))
        #expect(describe("telemetry", ["events": [1, 2]]).text.hasPrefix("telemetry: 2 event(s) dropped"))
        #expect(describe("settings.changed", ["key": "mode", "value": "compact"]).text.contains("settings.json is untouched"))
        #expect(describe("focus", ["slot": 3]).text.hasPrefix("focus:"))
        #expect(describe("future.op", ["a": 1]).text == "future.op {\"a\":1}")
    }
}
