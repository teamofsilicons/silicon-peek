import Foundation
import Observation
import OSLog
import PeekCore

/// One line of the Simulation log.
public struct SimulationLogEntry: Identifiable, Equatable, Sendable {
    public enum Kind: String, Sendable {
        /// What Simulation did.
        case info
        /// `peek.log` output of the drawing.
        case drawing
        /// A request the bubble sent where a real one would talk to peekd.
        case request
        case error
    }

    public let id: Int
    public let date: Date
    public let kind: Kind
    public let text: String
}

/// Runs Simulation (BLUEPRINT §8.11, visual.md B11): presents real bubbles with sample data.
///
/// For each run it
/// 1. starts (once) a dedicated presenter, a second ``PeekCoordinator`` wired to a
///    ``SimulationLink`` instead of peekd, a ``SimulationInputHub`` with the toggles' appearance
///    and context, a ``SimulatedBackdrop`` and the live drawing runtime;
/// 2. gives it a one-row slot table (context `simulation`, no hotkey) and loads the bundled
///    cassette through the presenter's own `drawing.load` handler;
/// 3. calls ``PeekPresenting/present(_:)`` with the scenario's `peek.show` event; and
/// 4. when Speak is on, streams synthesized PCM as `tts.begin` / `tts.chunk` / `tts.end` events,
///    which the presenter hands to its speech player, driving `speech.level/progress/done`.
///
/// Everything the bubble sends back (answers, dismissals, `speech.done`, voice recordings, …)
/// lands in ``log``. Nothing reaches peekd, IAM, Ting or Deepgram.
@MainActor
@Observable
public final class SimulationEngine {
    public enum Status: Equatable, Sendable {
        case idle
        case preparing
        case presenting(sendID: String, slot: SlotIndex)
        case closed(String)
        case failed(String)

        public var isActive: Bool {
            switch self {
            case .preparing, .presenting: true
            default: false
            }
        }
    }

    /// The Silicon every simulated bubble belongs to.
    public static let siliconKey = SiliconKey(context: .simulation, orgID: "simulation", actorID: "si:simulation")
    public static let displayName = "Simulation"
    public static let maxLogEntries = 500

    /// What the next ``simulate()`` presents; edited by the Simulation window.
    public var scenario: SimulationScenario
    public private(set) var status: Status = .idle
    public private(set) var log: [SimulationLogEntry] = []
    /// The speech player's state for the bubble on screen (level, progress, done).
    public private(set) var playback: SpeechPlayback?
    /// The bubble's `input.phase`, as the presenter last reported it.
    public private(set) var phase: Phase?
    public private(set) var drawingStatus: DrawingHostStatus?
    /// Also receives every log line (the launch hook writes them to stderr).
    @ObservationIgnored public var echo: (@MainActor (SimulationLogEntry) -> Void)?

    @ObservationIgnored public let dependencies: SimulationDependencies
    @ObservationIgnored private var pipeline: SimulationPipeline?
    @ObservationIgnored private var assets: SimulationAssetPaths?
    @ObservationIgnored private var loadedDrawingSHA: String?
    @ObservationIgnored private var currentSendID: String?
    @ObservationIgnored private var recordTask: Task<Void, Never>?
    @ObservationIgnored private var streamTask: Task<Void, Never>?
    @ObservationIgnored private var pollTask: Task<Void, Never>?
    /// Queue updates and Escs a 0.1.2 scenario performs after its bubble appears.
    @ObservationIgnored private var followUpTask: Task<Void, Never>?
    @ObservationIgnored private var pcmCache: [String: SimulationSpeechSynth.Clip] = [:]
    @ObservationIgnored private var nextLogID = 0
    @ObservationIgnored private let logger = PeekLogger(category: "simulation")

    public init(dependencies: SimulationDependencies, scenario: SimulationScenario = SimulationPreset.showCover.scenario) {
        self.dependencies = dependencies
        self.scenario = scenario
    }

    // MARK: Shared instance

    private static var instances: [ObjectIdentifier: SimulationEngine] = [:]

    /// The engine for `coordinator` if one was created.
    public static func existing(for coordinator: PeekCoordinator) -> SimulationEngine? {
        instances[ObjectIdentifier(coordinator)]
    }

    /// The app's engine for `coordinator` (created on first use; the pipeline itself starts lazily).
    public static func shared(for coordinator: PeekCoordinator) -> SimulationEngine {
        let id = ObjectIdentifier(coordinator)
        if let engine = instances[id] { return engine }
        let engine = SimulationEngine(dependencies: .live(for: coordinator))
        instances[id] = engine
        return engine
    }

    // MARK: Running

    /// Why the current ``scenario`` cannot be simulated, or nil.
    public var validationError: String? {
        do throws(SimulationScenarioError) {
            try scenario.validate()
            return nil
        } catch {
            return error.description
        }
    }

    public var isRunning: Bool { pipeline != nil }

    /// Presents ``scenario``.
    @discardableResult
    public func simulate() async -> Bool { await simulate(scenario) }

    /// Presents `scenario`, replacing a simulated bubble already on screen. Returns whether it was presented.
    @discardableResult
    public func simulate(_ scenario: SimulationScenario) async -> Bool {
        do throws(SimulationScenarioError) {
            try scenario.validate()
        } catch {
            fail(error.description)
            return false
        }
        status = .preparing
        let pipeline = await ensurePipeline(mode: scenario.mode)
        dismissCurrent(reason: "simulation_replaced")

        let assets: SimulationAssetPaths
        do throws(SimulationAssets.Failure) {
            assets = try await ensureAssets()
        } catch {
            fail(error.description)
            return false
        }

        pipeline.setMode(scenario.mode)
        pipeline.hub.setOverrides(appearance: Self.appearance(scenario.appearance), context: scenario.context)
        pipeline.backdrop.setTone(Self.tone(scenario.backdropTone))

        let key = Self.siliconKey
        let slot = SlotState(
            index: scenario.position, context: key.context, actorID: key.actorID, orgID: key.orgID,
            displayName: Self.displayName, initial: "S",
            drawing: DrawingRef(sha256: assets.cassetteSHA256, path: assets.cassette.path), hotkey: false)
        pipeline.presenter.updateSlots([slot])
        if loadedDrawingSHA != assets.cassetteSHA256 {
            await loadDrawing(assets: assets, slot: scenario.position, pipeline: pipeline)
        }

        let sendID = SimulationScenario.newID("snd")
        let event: PeekShowEvent
        do throws(SimulationScenarioError) {
            event = try scenario.makeEvent(assets: assets, sendID: sendID, askID: SimulationScenario.newID("ask"))
        } catch {
            fail(error.description)
            return false
        }
        currentSendID = sendID
        playback = nil
        phase = nil
        pipeline.presenter.present(event)
        status = .presenting(sendID: sendID, slot: scenario.position)
        append(.info, Self.describe(event, scenario: scenario))
        if let speak = event.speak { startSpeech(sendID: sendID, text: speak.text, pipeline: pipeline) }
        startPolling(pipeline: pipeline)
        startFollowUps(scenario, sendID: sendID, pipeline: pipeline)
        return true
    }

    // MARK: peek 0.1.2 follow-ups (queue badge, Esc on the ask)

    /// After the bubble is up: the scenario's `queue.state` (the badge's live update) and the Escs that collapse the ask
    /// and show "Esc again to dismiss" (pressed again whenever it fades, so screenshots can catch it).
    private func startFollowUps(_ scenario: SimulationScenario, sendID: String, pipeline: SimulationPipeline) {
        followUpTask?.cancel()
        guard scenario.queueUpdate != nil || scenario.askEsc != .none,
              let manager = (pipeline.presenter as? PeekCoordinator)?.slotManager else { return }
        let slot = scenario.position
        followUpTask = Task { @MainActor [weak self] in
            @MainActor func visible() -> Bool {
                manager.machine(on: slot)?.event?.sendID == sendID && manager.machine(on: slot)?.stage == .visible
            }
            var waited = 0
            while !visible() {
                try? await Task.sleep(for: .milliseconds(50))
                waited += 1
                if Task.isCancelled || waited > 200 { return }
            }
            if scenario.askEsc != .none {
                try? await Task.sleep(for: .milliseconds(400))
                guard !Task.isCancelled else { return }
                manager.escapePressed(fromKeySlot: nil)
                self?.append(.info, "pressed Esc: the ask collapsed to the compact ask (^ expands it)")
            }
            if let waiting = scenario.queueUpdate {
                try? await Task.sleep(for: SimulationScenario.queueUpdateDelay)
                guard !Task.isCancelled else { return }
                manager.applyQueueState(QueueStateEvent(slot: slot, context: .simulation, sendID: sendID, waiting: waiting))
                self?.append(.info, "queue.state: \(waiting) waiting behind \(sendID) (the badge updates live)")
            }
            guard scenario.askEsc == .hint else { return }
            let timing = manager.timing
            try? await Task.sleep(for: .milliseconds(Int((timing.escDouble + 0.2) * 1000)))
            while !Task.isCancelled, manager.machine(on: slot)?.event?.sendID == sendID {
                if manager.machine(on: slot)?.escHintVisible == false, manager.machine(on: slot)?.escArmedUntil == nil {
                    manager.escapePressed(fromKeySlot: nil)
                    self?.append(.info, "pressed Esc on the compact ask: \"\(BubbleMachine.escAgainHint)\" shows for "
                        + "\(timing.escHint) s")
                }
                try? await Task.sleep(for: .milliseconds(Int((timing.escHint + 0.15) * 1000)))
            }
        }
    }

    /// Slides the simulated bubble out and stops its speech.
    public func stop() {
        guard currentSendID != nil else { return }
        dismissCurrent(reason: "simulation_stopped")
        status = .idle
        append(.info, "stopped the simulation")
    }

    /// Stops and tears the presenter down (the Simulation window closed).
    public func shutdown() async {
        dismissCurrent(reason: "simulation_closed")
        recordTask?.cancel()
        pollTask?.cancel()
        recordTask = nil
        pollTask = nil
        guard let pipeline else { return }
        self.pipeline = nil
        pipeline.presenter.dismissAll()
        await pipeline.presenter.stop()
        await pipeline.link.stop()
        pipeline.drawing.unloadAll(for: Self.siliconKey)
        pipeline.hub.remove(Self.siliconKey)
        loadedDrawingSHA = nil
        status = .idle
        playback = nil
        phase = nil
        drawingStatus = nil
    }

    public func clearLog() { log.removeAll() }

    /// The log as plain text (Copy button).
    public var logText: String {
        let formatter = DateFormatter()
        formatter.dateFormat = "HH:mm:ss.SSS"
        return log.map { "\(formatter.string(from: $0.date)) [\($0.kind.rawValue)] \($0.text)" }.joined(separator: "\n")
    }

    // MARK: Pipeline

    private func ensurePipeline(mode: DisplayMode) async -> SimulationPipeline {
        if let pipeline { return pipeline }
        let pipeline = SimulationPipeline(dependencies: dependencies, mode: mode)
        self.pipeline = pipeline
        pipeline.drawing.onLog = { [weak self] _, line in self?.append(.drawing, line) }
        pipeline.drawing.onFailure = { [weak self] _, failure in
            self?.append(.error, "the drawing switched to the fallback visual (\(failure.reason.rawValue)): \(failure.message)")
        }
        pipeline.hub.onStateChange = { [weak self] key, state in
            guard let self, key == Self.siliconKey else { return }
            self.phase = state.phase
        }
        let records = pipeline.link.records
        recordTask = Task { [weak self] in
            for await record in records {
                self?.handle(record)
            }
        }
        await pipeline.presenter.start()
        append(.info, "started the Simulation presenter (its own coordinator; requests go to Simulation, never to peekd)")
        return pipeline
    }

    private func ensureAssets() async throws(SimulationAssets.Failure) -> SimulationAssetPaths {
        if let assets { return assets }
        let directory = dependencies.assetsDirectory
        let result = await Task.detached(priority: .userInitiated) { () -> Result<SimulationAssetPaths, SimulationAssets.Failure> in
            do throws(SimulationAssets.Failure) {
                return .success(try SimulationAssets.materialize(into: directory))
            } catch {
                return .failure(error)
            }
        }.value
        switch result {
        case .success(let paths):
            assets = paths
            append(.info, "sample images and cassette.js are in \(paths.directory.path)")
            return paths
        case .failure(let failure):
            throw failure
        }
    }

    private func loadDrawing(assets: SimulationAssetPaths, slot: SlotIndex, pipeline: SimulationPipeline) async {
        let key = Self.siliconKey
        let request = DrawingLoadRequest(
            context: key.context, orgID: key.orgID, actorID: key.actorID, slot: slot, scriptPath: assets.cassette.path,
            sha256: assets.cassetteSHA256)
        switch await pipeline.link.request(.drawingLoad(request)) {
        case .ok:
            loadedDrawingSHA = assets.cassetteSHA256
            append(.info, "loaded cassette.js (sha256 \(assets.cassetteSHA256.prefix(12))…) into the drawing runtime")
        case .failure(let error):
            append(
                .error,
                "drawing.load failed (\(error.code)): \(error.message). The bubble shows the fallback visual instead of the cassette.")
        }
        drawingStatus = pipeline.drawing.host(for: key)?.status
    }

    private func dismissCurrent(reason: String) {
        streamTask?.cancel()
        streamTask = nil
        followUpTask?.cancel()
        followUpTask = nil
        guard let sendID = currentSendID, let pipeline else {
            currentSendID = nil
            return
        }
        currentSendID = nil
        pipeline.speech.stop(sendID: sendID)
        pipeline.presenter.cancel(sendID: sendID, reason: .other(reason))
    }

    // MARK: Speech

    private func startSpeech(sendID: String, text: String, pipeline: SimulationPipeline) {
        streamTask?.cancel()
        let link = pipeline.link
        let interval = dependencies.chunkInterval
        streamTask = Task { [weak self] in
            guard let clip = await self?.clip(for: text), !Task.isCancelled else { return }
            // peekd's estimate before tts.end: characters ÷ 14 per second (BLUEPRINT §8.7).
            let estimated = Int(Double(text.scalarCount) / 14 * Double(SimulationSpeechSynth.sampleRate))
            await link.push(.ttsBegin(TTSBegin(sendID: sendID, estFrames: estimated)))
            let chunkBytes = Self.chunkFrames * 2
            var seq = 0
            var offset = 0
            while offset < clip.pcm.count {
                guard !Task.isCancelled else { return }
                let end = min(offset + chunkBytes, clip.pcm.count)
                await link.push(.ttsChunk(TTSChunk(sendID: sendID, seq: seq, pcm: clip.pcm.subdata(in: offset..<end))))
                seq += 1
                offset = end
                try? await Task.sleep(for: interval)
            }
            guard !Task.isCancelled else { return }
            await link.push(.ttsEnd(TTSEnd(sendID: sendID, totalFrames: clip.frames)))
            self?.append(
                .info,
                "streamed \(seq) PCM chunks (\(Self.seconds(clip.durationMs)) of 24 kHz s16le mono) to the speech player")
        }
    }

    /// 200 ms of 24 kHz audio per `tts.chunk` (9,600 bytes, well under the 64 KiB limit).
    nonisolated static let chunkFrames = 4_800

    private func clip(for text: String) async -> SimulationSpeechSynth.Clip {
        if let cached = pcmCache[text] { return cached }
        let clip = await Task.detached(priority: .userInitiated) { SimulationSpeechSynth.synthesize(text) }.value
        if pcmCache.count >= 8 { pcmCache.removeAll() }
        pcmCache[text] = clip
        return clip
    }

    // MARK: Polling (the window's speech meter and phase)

    private func startPolling(pipeline: SimulationPipeline) {
        pollTask?.cancel()
        pollTask = Task { [weak self, weak pipeline] in
            while !Task.isCancelled {
                guard let self, let pipeline else { return }
                self.refresh(from: pipeline)
                if !self.status.isActive, self.playback?.done ?? true { return }
                try? await Task.sleep(for: .milliseconds(100))
            }
        }
    }

    private func refresh(from pipeline: SimulationPipeline) {
        let key = Self.siliconKey
        if case .presenting(let sendID, _) = status {
            playback = pipeline.speech.playback(for: sendID)
        }
        if let state = pipeline.hub.state(for: key) { phase = state.phase }
        drawingStatus = pipeline.drawing.host(for: key)?.status ?? drawingStatus
    }

    // MARK: Requests from the bubble

    private func handle(_ record: SimulationRequestRecord) {
        let description = SimulationRequestDescriber.describe(record)
        append(description.isError ? .error : .request, description.text)
        guard case .presenting(let sendID, _) = status, record.fields["send_id"]?.stringValue == sendID else { return }
        if let closing = description.closesBubble {
            status = .closed(closing)
        }
    }

    // MARK: Log

    func append(_ kind: SimulationLogEntry.Kind, _ text: String) {
        let entry = SimulationLogEntry(id: nextLogID, date: Date(), kind: kind, text: text)
        nextLogID += 1
        log.append(entry)
        if log.count > Self.maxLogEntries { log.removeFirst(log.count - Self.maxLogEntries) }
        switch kind {
        case .error: logger.error("\(text)")
        case .info: logger.info("\(text)")
        // Requests can carry what the Carbon typed: keep them out of ui.log (debug stays in memory only).
        case .request: logger.debug("\(text)")
        case .drawing: logger.debug("peek.log: \(text)")
        }
        echo?(entry)
    }

    private func fail(_ message: String) {
        status = .failed(message)
        append(.error, message)
    }

    // MARK: Helpers

    static func appearance(_ choice: SimulationScenario.AppearanceChoice) -> Appearance? {
        switch choice {
        case .system: nil
        case .light: .light
        case .dark: .dark
        }
    }

    static func tone(_ choice: SimulationScenario.BackdropChoice) -> SimulatedBackdrop.Tone {
        switch choice {
        case .light: .fixed(.light)
        case .dark: .fixed(.dark)
        case .live: .live
        }
    }

    nonisolated static func seconds(_ milliseconds: Int) -> String { String(format: "%.1f s", Double(milliseconds) / 1000) }

    static func describe(_ event: PeekShowEvent, scenario: SimulationScenario) -> String {
        var parts: [String] = []
        if event.speak != nil { parts.append("speak") }
        if let show = event.show { parts.append("show (\(show.elements.count) element\(show.elements.count == 1 ? "" : "s"))") }
        if let ask = event.ask { parts.append("ask \(ask.type.rawValue)" + (ask.imagePaths.isEmpty ? "" : " with images")) }
        return "presented \(event.sendID) at position \(event.slot.rawValue) (\(event.slot.side.rawValue)): "
            + parts.joined(separator: " + ")
            + "; mode \(scenario.mode.rawValue), appearance \(scenario.appearance.rawValue), backdrop \(scenario.backdropTone.rawValue), "
            + "input.context \(scenario.context.rawValue)"
    }
}

/// Turns a recorded request into a log line, saying what peekd and the Silicon would have done.
enum SimulationRequestDescriber {
    struct Description: Equatable {
        var text: String
        var isError = false
        /// Set when the request means the bubble left the screen.
        var closesBubble: String?
    }

    static func describe(_ record: SimulationRequestRecord) -> Description {
        let f = record.fields
        switch record.op {
        case AnswerRequest.op:
            let value = f["value"]?.jsonString ?? "null"
            let via = f["via"]?.stringValue ?? "?"
            return Description(
                text: "answer \(value) via \(via) → a Silicon would receive a peek.ask.answered ting (Simulation sends none)",
                closesBubble: "answered \(value) via \(via)")
        case DismissedRequest.op:
            let gesture = f["gesture"]?.stringValue ?? "?"
            return Description(text: "dismissed with \(gesture)", closesBubble: "dismissed (\(gesture))")
        case SpeechDoneRequest.op:
            let played = f["played_ms"]?.intValue ?? 0
            let total = f["total_ms"]?.intValue ?? 0
            let stopped = f["stopped_by_user"]?.boolValue == true
            return Description(
                text: "speech.done: played \(SimulationEngine.seconds(played)) of \(SimulationEngine.seconds(total))"
                    + (stopped ? ", stopped by a double click" : ""))
        case ShownDoneRequest.op:
            let visible = f["visible_ms"]?.intValue ?? 0
            let reason = f["reason"]?.stringValue ?? "?"
            return Description(
                text: "shown.done after \(SimulationEngine.seconds(visible)) (\(reason))",
                closesBubble: "slid out after \(SimulationEngine.seconds(visible)) (\(reason))")
        case VoiceSubmitRequest.op:
            let duration = f["duration_ms"]?.intValue ?? 0
            let bytes = record.blobSizes.reduce(0, +)
            return Description(
                text: "voice.submit: \(SimulationEngine.seconds(duration)) WAV (\(ByteCountFormatter.string(fromByteCount: Int64(bytes), countStyle: .file))) "
                    + "discarded. Simulation never sends audio to Deepgram; it answered with stt.result \"failed\".")
        case MessageRequest.op:
            let text = f["text"]?.stringValue ?? ""
            return Description(text: "message \"\(text)\" → a Silicon would receive peek.message.received (Simulation sends none)")
        case FocusRequest.op:
            return Description(text: "focus: peekd would pre-warm the Silicon's session and the Deepgram token")
        case DrawingErrorRequest.op:
            let reason = f["reason"]?.stringValue ?? "?"
            let message = f["message"]?.stringValue ?? ""
            return Description(text: "drawing.error (\(reason)): \(message)", isError: true)
        case TelemetryRequest.op:
            let count = f["events"]?.arrayValue?.count ?? 0
            return Description(text: "telemetry: \(count) event(s) dropped (Simulation sends no telemetry)")
        case SettingsChangedRequest.op:
            let key = f["key"]?.stringValue ?? "?"
            return Description(text: "settings.changed \(key) = \(f["value"]?.jsonString ?? "null") (Simulation only; settings.json is untouched)")
        default:
            return Description(text: "\(record.op) \(record.fields.jsonString)")
        }
    }
}
