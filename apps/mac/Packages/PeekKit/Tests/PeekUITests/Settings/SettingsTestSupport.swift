import AppKit
import Foundation
import PeekCore
import Testing

@testable import PeekUI

// Fakes shared by the ui-settings test files. Every name starts with `SettingsSuite` so it cannot
// collide with the ui-slots tests in the same module.

/// A temporary `PeekPaths` home, removed by the caller.
func settingsSuiteTemporaryPaths() -> PeekPaths {
    PeekPaths(home: FileManager.default.temporaryDirectory.appendingPathComponent("peek-settings-\(UUID().uuidString)"))
}

func settingsSuiteRemove(_ paths: PeekPaths) {
    try? FileManager.default.removeItem(at: paths.home)
}

/// Polls `condition` on the main actor until it holds or `timeout` passes.
@MainActor
func settingsSuiteWait(timeout: Duration = .seconds(5), _ condition: @MainActor () async -> Bool) async -> Bool {
    let deadline = ContinuousClock.now + timeout
    while ContinuousClock.now < deadline {
        if await condition() { return true }
        try? await Task.sleep(for: .milliseconds(10))
    }
    return await condition()
}

/// An in-memory peekd link: records every request, acknowledges it.
actor SettingsSuiteLink: DaemonLinking {
    private(set) var sent: [(op: String, fields: JSONValue)] = []
    private var eventContinuations: [AsyncStream<DaemonEvent>.Continuation] = []
    private var handler: (@Sendable (DaemonRequest) async -> DaemonReply)?

    func start() {}
    func stop() { for continuation in eventContinuations { continuation.finish() } }

    func events() -> AsyncStream<DaemonEvent> {
        let (stream, continuation) = AsyncStream.makeStream(of: DaemonEvent.self)
        eventContinuations.append(continuation)
        return stream
    }

    func states() -> AsyncStream<DaemonLinkState> {
        let (stream, continuation) = AsyncStream.makeStream(of: DaemonLinkState.self)
        continuation.yield(.idle)
        return stream
    }

    func send<R: UIRequest>(_ request: R, blobs: [Data], timeout: Duration?) async throws(DaemonLinkError) -> R.Reply {
        do {
            sent.append((R.op, .object(try FrameCoding.fields(of: request))))
            return try FrameCoding.decode(R.Reply.self, from: [:])
        } catch {
            throw .invalidRequest(op: R.op, reason: error.description)
        }
    }

    func setRequestHandler(_ handler: (@Sendable (DaemonRequest) async -> DaemonReply)?) { self.handler = handler }

    func settingsChanges() -> [(key: String, value: JSONValue)] {
        sent.filter { $0.op == SettingsChangedRequest.op }.compactMap { entry in
            guard let key = entry.fields["key"]?.stringValue, let value = entry.fields["value"] else { return nil }
            return (key, value)
        }
    }
}

/// `PeekControlling` without a coordinator: applies settings in memory and records each call.
@MainActor
final class SettingsSuiteControls: PeekControlling {
    var linkState: DaemonLinkState = .idle
    var slots: [SlotState] = []
    var settings = PeekSettings()
    var settingsWarnings: [String] = []
    var paused = false
    var lastProblem: String?
    var hotkeyProblems: [String] = []
    let paths: PeekPaths
    let drawing: any DrawingRuntimeProviding = SettingsSuiteDrawingRuntime()
    let mic: any MicRecording = SettingsSuiteMic()
    private(set) var applied: [(PeekSettings.Key, JSONValue)] = []

    init(paths: PeekPaths = settingsSuiteTemporaryPaths()) { self.paths = paths }

    func setSetting(_ key: PeekSettings.Key, _ value: JSONValue) {
        applied.append((key, value))
        do throws(SettingsError) {
            try settings.apply(key, value)
        } catch {
            lastProblem = error.description
        }
    }
}

@MainActor
final class SettingsSuiteLoginItems: LoginItemStatusProviding {
    var status = LoginItemStatus(app: .enabled, helper: .requiresApproval)
    private(set) var openedSettings = 0
    func currentStatus() -> LoginItemStatus { status }
    func openLoginItemsSettings() { openedSettings += 1 }
}

@MainActor
final class SettingsSuiteScreenCapture: ScreenCapturePermissionProviding {
    var granted = false
    var grantOnRequest = false
    private(set) var requests = 0
    func isGranted() -> Bool { granted }
    func request() -> Bool {
        requests += 1
        if grantOnRequest { granted = true }
        return granted
    }
    func openSystemSettings() {}
}

// MARK: - Pipeline fakes

@MainActor
final class SettingsSuiteDrawingRuntime: DrawingRuntimeProviding {
    let glassMode: GlassMode = .frosted
    private(set) var madeHosts: [SettingsSuiteDrawingHost] = []

    func makeHost(for key: SiliconKey, initial: String, images: any ImageProviding) -> any DrawingHosting {
        if let host = madeHosts.first(where: { $0.key == key }) { return host }
        let host = SettingsSuiteDrawingHost(key: key)
        madeHosts.append(host)
        return host
    }

    func validate(scriptAt url: URL, options: ValidationOptions) async -> ValidationReport { ValidationReport(ok: true) }
}

@MainActor
final class SettingsSuiteDrawingHost: DrawingHosting {
    let key: SiliconKey
    private(set) var status: DrawingHostStatus = .empty
    var input: (any DrawingInputSource)?
    var onFailure: (@MainActor (DrawingFailure) -> Void)?
    var onLog: (@MainActor (String) -> Void)?
    private(set) var loaded: DrawingScript?
    private(set) var unloads = 0

    init(key: SiliconKey) { self.key = key }

    func load(_ script: DrawingScript) async throws(DrawingFailure) {
        loaded = script
        status = .ready(sha256: script.sha256)
        onLog?("loaded \(script.filename) (\(script.source.count) bytes)")
    }

    func unload() {
        unloads += 1
        status = .empty
    }

    func attach(to visualView: NSView) {}
    func detach() {}
    func deliver(_ event: DrawingEvent) { onLog?("event \(event.name)") }
    func wake() {}
    func isOverContent(unitPoint: CGPoint) -> Bool { false }
    func validate(_ script: DrawingScript, options: ValidationOptions) async -> ValidationReport { ValidationReport(ok: true) }
}

@MainActor
final class SettingsSuiteSpeech: SpeechPlaying {
    var onFinished: (@MainActor (SpeechFinished) -> Void)?
    var onFailed: (@MainActor (String, IPCErrorBody) -> Void)?
    private(set) var events: [TTSStreamEvent] = []
    private(set) var stopped: [String] = []

    func handle(_ event: TTSStreamEvent) { events.append(event) }
    func stop(sendID: String) { stopped.append(sendID) }

    func playback(for sendID: String) -> SpeechPlayback? {
        let received = events.reduce(0) { total, event in
            if case .chunk(let chunk) = event, chunk.sendID == sendID { return total + chunk.pcm.count / 2 }
            return total
        }
        guard received > 0 else { return nil }
        let total = events.compactMap { event -> Int? in
            if case .end(let end) = event, end.sendID == sendID { return end.totalFrames }
            return nil
        }.first
        let progress = total.map { Double(received) / Double(max($0, 1)) } ?? 0.5
        return SpeechPlayback(
            sendID: sendID, level: 0.4, progress: progress, done: total != nil, started: true, playedMs: received / 24,
            totalMs: total.map { $0 / 24 })
    }
}

@MainActor
final class SettingsSuiteMic: MicRecording {
    var permission: MicPermission = .undetermined
    var isRecording = false
    var level: Double = 0
    var onAutoStop: (@MainActor (MicRecordingResult) -> Void)?
    func requestPermission() async -> Bool { false }
    func start() throws(MicRecordingError) { throw MicRecordingError("the test mic cannot record") }
    func stop() async throws(MicRecordingError) -> MicRecordingResult { throw MicRecordingError("not recording") }
    func cancel() {}
}

@MainActor
final class SettingsSuiteImages: ImageProviding {
    private(set) var prepared: [String] = []
    func prepare(sendID: String, paths: [String]) async -> [String: PreparedImage] {
        prepared.append(contentsOf: paths)
        return [:]
    }
    func image(for handle: ImageHandle) -> CGImage? { nil }
    func release(sendID: String) {}
}

@MainActor
final class SettingsSuiteBackdrop: BackdropSampling {
    var source: BackdropSourceSetting = .wallpaper
    var onChange: (@MainActor (SiliconKey, Backdrop) -> Void)?
    func track(_ key: SiliconKey, rectOnScreen: CGRect?) {}
    func backdrop(for key: SiliconKey) -> Backdrop { .fromAppearance(.light) }
}

@MainActor
final class SettingsSuiteHub: InputHubbing {
    let appearance: Appearance = .light
    let backdrop: any BackdropSampling
    private var states: [SiliconKey: BubbleInputState] = [:]
    private var sources: [SiliconKey: SettingsSuiteSource] = [:]

    init(backdrop: any BackdropSampling) { self.backdrop = backdrop }

    func source(for key: SiliconKey) -> any DrawingInputSource {
        if let source = sources[key] { return source }
        let source = SettingsSuiteSource(key: key, hub: self)
        sources[key] = source
        return source
    }

    func update(_ key: SiliconKey, _ mutate: (inout BubbleInputState) -> Void) {
        var state = states[key] ?? BubbleInputState(slot: .top)
        mutate(&state)
        states[key] = state
    }

    func state(for key: SiliconKey) -> BubbleInputState? { states[key] }

    func remove(_ key: SiliconKey) {
        states.removeValue(forKey: key)
        sources.removeValue(forKey: key)
    }

    func snapshot(for key: SiliconKey, t: Double, dt: Double) -> InputSnapshot {
        let state = states[key] ?? BubbleInputState(slot: .top)
        return InputSnapshot(
            t: t, dt: dt, slot: .init(index: state.slot, facing: state.facing), mode: state.mode, appearance: appearance,
            backdrop: backdrop.backdrop(for: key), phase: state.phase, context: state.context, glass: state.glass)
    }
}

@MainActor
final class SettingsSuiteSource: DrawingInputSource {
    let key: SiliconKey
    weak var hub: SettingsSuiteHub?
    var onWake: (@MainActor (WakeReason) -> Void)?

    init(key: SiliconKey, hub: SettingsSuiteHub) {
        self.key = key
        self.hub = hub
    }

    func snapshot(t: Double, dt: Double) -> InputSnapshot {
        hub?.snapshot(for: key, t: t, dt: dt) ?? InputSnapshot(slot: .init(index: .top, facing: 0))
    }
}

/// A presenter that behaves like the coordinator at its seams: it subscribes to the link, answers
/// `drawing.load` by loading through the drawing runtime, and records every PeekPresenting call.
@MainActor
final class SettingsSuitePresenter: SimulationPresenter {
    let parts: SimulationPresenterParts
    private(set) var started = 0
    private(set) var stopped = 0
    private(set) var slotTables: [[SlotState]] = []
    private(set) var presented: [PeekShowEvent] = []
    private(set) var cancelled: [(String, PeekCancelReason)] = []
    private(set) var dismissAllCount = 0
    private(set) var settingsChanges: [(PeekSettings.Key, JSONValue)] = []
    private(set) var events: [DaemonEvent] = []
    private(set) var loads: [DrawingLoadRequest] = []
    private var eventTask: Task<Void, Never>?

    init(parts: SimulationPresenterParts) { self.parts = parts }

    func start() async {
        started += 1
        let events = await parts.link.events()
        eventTask = Task { [weak self] in
            for await event in events {
                guard let self else { return }
                self.events.append(event)
                switch event {
                case .ttsBegin(let e): self.parts.speech.handle(.begin(e))
                case .ttsChunk(let e): self.parts.speech.handle(.chunk(e))
                case .ttsEnd(let e): self.parts.speech.handle(.end(e))
                default: break
                }
            }
        }
        await parts.link.setRequestHandler { [weak self] request in
            guard case .drawingLoad(let load) = request else { return .failure(.unknownOp(request.op)) }
            return await self?.load(load) ?? .failure(.unknownOp(request.op))
        }
        await parts.link.start()
    }

    private func load(_ request: DrawingLoadRequest) async -> DaemonReply {
        loads.append(request)
        let host = parts.drawing.makeHost(for: request.siliconKey, initial: "S", images: parts.images)
        host.input = parts.input.source(for: request.siliconKey)
        guard let data = FileManager.default.contents(atPath: request.scriptPath) else {
            return .failure(IPCErrorBody(code: "drawing_unreadable", message: "cannot read \(request.scriptPath)"))
        }
        do throws(DrawingFailure) {
            try await host.load(
                DrawingScript(key: request.siliconKey, sha256: request.sha256, source: data, filename: "cassette.js"))
            return .encode(DrawingLoadResult(ok: true))
        } catch {
            return .failure(IPCErrorBody(code: "drawing_load_failed", message: error.message))
        }
    }

    func stop() async {
        stopped += 1
        eventTask?.cancel()
    }

    func setSetting(_ key: PeekSettings.Key, _ value: JSONValue) { settingsChanges.append((key, value)) }
    func updateSlots(_ slots: [SlotState]) { slotTables.append(slots) }

    func present(_ show: PeekShowEvent) {
        presented.append(show)
        parts.input.update(SimulationEngine.siliconKey) { state in
            state.slot = show.slot
            state.phase = .entering
            state.sendID = show.sendID
        }
    }

    func cancel(sendID: String, reason: PeekCancelReason) { cancelled.append((sendID, reason)) }
    func applySTTResult(_ result: STTResultEvent) {}
    var isIdleForUpdate: Bool { presented.isEmpty }
    func dismissAll() { dismissAllCount += 1 }

    /// Sends a request the way a bubble would (answer, dismissed, voice.submit, …).
    func send<R: UIRequest>(_ request: R, blobs: [Data] = []) async throws(DaemonLinkError) {
        _ = try await parts.link.send(request, blobs: blobs, timeout: nil)
    }
}

/// Simulation dependencies built entirely from fakes; `presenters` collects what `makePresenter` built.
@MainActor
final class SettingsSuitePipelineFactory {
    let paths = settingsSuiteTemporaryPaths()
    let drawing = SettingsSuiteDrawingRuntime()
    private(set) var presenters: [SettingsSuitePresenter] = []
    private(set) var speeches: [SettingsSuiteSpeech] = []
    private(set) var hubs: [SettingsSuiteHub] = []

    var dependencies: SimulationDependencies {
        var dependencies = SimulationDependencies(
            paths: paths, baseSettings: { PeekSettings() }, drawing: drawing,
            makeSpeech: { [unowned self] in
                let speech = SettingsSuiteSpeech()
                speeches.append(speech)
                return speech
            },
            makeMic: { SettingsSuiteMic() }, makeImages: { SettingsSuiteImages() },
            makeLiveBackdrop: { SettingsSuiteBackdrop() },
            makeInputHub: { [unowned self] _, _, _, backdrop in
                let hub = SettingsSuiteHub(backdrop: backdrop)
                hubs.append(hub)
                return hub
            },
            makePresenter: { [unowned self] parts in
                let presenter = SettingsSuitePresenter(parts: parts)
                presenters.append(presenter)
                return presenter
            })
        dependencies.chunkInterval = .zero
        return dependencies
    }

    deinit {
        try? FileManager.default.removeItem(at: paths.home)
    }
}
