import AppKit
import Observation
import OSLog
import PeekAudio
import PeekCore
import PeekDrawing
import PeekIPC
import PeekInput

/// How the coordinator puts bubbles on screen.
public enum PresentationMode: Sendable, Equatable {
    /// Real slot panels when running inside a GUI app (NSApp is running), windowless otherwise.
    case automatic
    /// Always real panels.
    case live
    /// Never windows (tests, GUI-less runs): bubbles run their full logic on ``HeadlessSlotSurface``s.
    case headless
}

/// The app's hub (visual.md B1, BLUEPRINT §8.2): owns the link to peekd, the 8 slots
/// (``SlotManager``), one drawing host per Silicon, the input hub, speech player, microphone and
/// the Carbon hotkeys. It routes peekd's events and requests, turns what the Carbon does into
/// requests to peekd, applies settings, and implements ``PeekPresenting`` so Simulation can drive
/// the same code without peekd.
@MainActor
@Observable
public final class PeekCoordinator: PeekPresenting {
    public private(set) var linkState: DaemonLinkState = .idle
    /// The slot table from the latest `slots.state` (every context), sorted by position.
    public private(set) var slots: [SlotState] = []
    /// Bubbles on screen or queued, by send id.
    public private(set) var visible: [String: PeekShowEvent] = [:]
    public private(set) var settings: PeekSettings
    public private(set) var settingsWarnings: [String]
    /// Silences Silicon-initiated peeks from the menu bar: new ones wait (the Carbon's hotkey still shows them).
    /// peekd hears it in `presence.paused` (peek 0.1.2), so it can say a send is held because Peek is paused.
    public var paused = false {
        didSet {
            applyGate()
            if paused != oldValue { sendPresence() }
        }
    }
    public private(set) var lastProblem: String?
    /// Shortcuts macOS refused (taken by another app, …).
    public private(set) var hotkeyProblems: [String] = []
    /// `live` when the private active-appearance override is in use (D13), else `frosted`.
    public private(set) var glassMode: GlassMode
    /// `peek.log` lines from Simulation drawings (newest last, at most 200).
    public private(set) var drawingLog: [String] = []
    /// Hotkeys registered right now (`ctrl+cmd+5` form), for `ui.status` and Settings.
    public private(set) var registeredHotkeys: [String] = []
    /// Hotkeys macOS refused (`ctrl+cmd+3` form).
    public private(set) var failedHotkeys: [String] = []
    /// Whether the Carbon can see bubbles (screen unlocked, displays awake), as last reported by the presence monitor.
    /// peekd gets it with ``paused`` merged in (`presence`).
    public private(set) var carbonPresence: PresenceRequest = .available

    /// Runs `peek app uninstall` (unregister the login item and agent, recycle the bundle, quit). Set by the app
    /// target, which owns SMAppService; without it `app.uninstall` is refused.
    @ObservationIgnored public var uninstallHandler: (@MainActor () async -> Void)?
    /// Whether an uninstall also unregisters the services (false under `PEEK_NO_SERVICES=1`).
    @ObservationIgnored public var servicesEnabled = true

    public let paths: PeekPaths
    @ObservationIgnored public let link: any DaemonLinking
    @ObservationIgnored public let drawing: any DrawingRuntimeProviding
    @ObservationIgnored public let speech: any SpeechPlaying
    @ObservationIgnored public let mic: any MicRecording
    @ObservationIgnored public let images: any ImageProviding
    @ObservationIgnored public let input: any InputHubbing
    @ObservationIgnored public let backdrop: any BackdropSampling
    @ObservationIgnored public private(set) var slotManager: SlotManager!
    @ObservationIgnored public let presentation: PresentationMode

    @ObservationIgnored private var tasks: [Task<Void, Never>] = []
    @ObservationIgnored private var hosts: [SiliconKey: any DrawingHosting] = [:]
    @ObservationIgnored private var drawingLoads: [SiliconKey: (sha256: String, task: Task<DrawingFailure?, Never>)] = [:]
    @ObservationIgnored private var hotKeys: HotKeyCenter?
    @ObservationIgnored private var speechDoneSent: Set<String> = []
    @ObservationIgnored private var telemetryBuffer: [TelemetryEvent] = []
    @ObservationIgnored private var reportedGlassMode = false
    @ObservationIgnored private var screenObserver: (any NSObjectProtocol)?
    @ObservationIgnored private var keyWindowObservers: [any NSObjectProtocol] = []
    /// peekd answered `unknown_op` to `shown` (an older build during an update swap): not sent again on this link.
    @ObservationIgnored private var shownUnsupported = false
    @ObservationIgnored private var presenceMonitor: PresenceMonitor?
    @ObservationIgnored private var presenceTask: Task<Void, Never>?
    @ObservationIgnored private var lastSentStatus: UIStatusReport?
    @ObservationIgnored private var statusUnsupported = false
    @ObservationIgnored private let settingsURL: URL?
    @ObservationIgnored private let logger = PeekLogger(category: "ui")

    static let telemetryFlushInterval: Duration = .seconds(30)
    static let telemetryBatch = 20

    public init(link: any DaemonLinking, drawing: any DrawingRuntimeProviding, speech: any SpeechPlaying,
                mic: any MicRecording, images: any ImageProviding, input: any InputHubbing,
                backdrop: any BackdropSampling, paths: PeekPaths, settings: PeekSettings = PeekSettings(),
                settingsWarnings: [String] = [], persistSettings: Bool = true, presentation: PresentationMode = .automatic,
                timing: BubbleTiming = BubbleTiming()) {
        self.link = link
        self.drawing = drawing
        self.speech = speech
        self.mic = mic
        self.images = images
        self.input = input
        self.backdrop = backdrop
        self.paths = paths
        self.settings = settings
        self.settingsWarnings = settingsWarnings
        self.settingsURL = persistSettings ? paths.settingsFile : nil
        self.presentation = presentation
        glassMode = GlassSupport.selfCheck()
        backdrop.source = settings.backdrop
        // The second click of a double click must land while the panel still takes it (the slide-out settles in 0.5 s).
        var timing = timing
        timing.doubleClick = min(timing.doubleClick, NSEvent.doubleClickInterval)
        slotManager = SlotManager(environment: makeEnvironment(timing: timing))
        slotManager.onSendsChanged = { [weak self] in
            guard let self else { return }
            let sends = self.slotManager.sends
            if self.visible != sends { self.visible = sends }
        }
        slotManager.onEscapeProblemChanged = { [weak self] _ in self?.updateHotkeyProblems() }
        slotManager.setMode(settings.mode)
        slotManager.setDisplay(settings.display)
        applyGate()
    }

    /// Whether bubbles use real panels right now.
    public var usesLivePanels: Bool {
        switch presentation {
        case .live: true
        case .headless: false
        case .automatic: NSApp?.isRunning == true
        }
    }

    private func makeEnvironment(timing: BubbleTiming) -> SlotManagerEnvironment {
        SlotManagerEnvironment(
            speech: speech, mic: mic, images: images, input: input, backdrop: backdrop, glassMode: glassMode, timing: timing,
            makeSurface: { [weak self] _, chrome in
                guard let self, self.usesLivePanels else { return HeadlessSlotSurface(chrome: chrome) }
                return SlotPanelController(chrome: chrome, glass: self.glassMode)
            },
            visibleFrame: { [weak self] target in
                guard let self, self.usesLivePanels else { return ScreenPolicy.fallbackVisibleFrame }
                return ScreenPolicy.visibleFrame(for: target)
            },
            host: { [weak self] key in self?.host(for: key) },
            send: { [weak self] context, outbound in
                guard let self else { return .failed("Peek.app is quitting") }
                return await self.send(outbound, context: context)
            },
            speechStoppedBeforeStart: { [weak self] sendID, byUser in
                self?.sendSpeechDone(SpeechFinished(sendID: sendID, stoppedByUser: byUser, playedMs: 0, totalMs: 0))
            },
            telemetry: { [weak self] type, data, context in self?.record(type, data, context: context) },
            // Bare Esc is only ever grabbed by the real app (never by tests or GUI-less runs).
            escapeKeys: usesLivePanels ? CarbonEscapeKey() : InertEscapeKey(),
            keyWindowIsOpen: { NSApp?.keyWindow != nil })
    }

    // MARK: Lifecycle

    /// Subscribes to the link and starts connecting. Idempotent.
    public func start() async {
        guard tasks.isEmpty else { return }
        installCallbacks()
        let link = self.link
        let states = await link.states()
        let events = await link.events()
        tasks.append(Task { [weak self] in
            for await state in states {
                guard let self else { return }
                self.linkState = state
                // A new peekd (or a restarted one) knows nothing yet: report again after every hello.
                if state.isConnected {
                    self.lastSentStatus = nil
                    self.statusUnsupported = false
                    self.shownUnsupported = false
                    self.pushStatusIfChanged()
                }
            }
        })
        tasks.append(Task { [weak self] in
            for await event in events { self?.handle(event) }
        })
        tasks.append(Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: Self.telemetryFlushInterval)
                guard !Task.isCancelled else { return }
                self?.flushTelemetry()
                // The mic permission can change in System Settings at any time.
                self?.pushStatusIfChanged()
            }
        })
        await link.setRequestHandler { [weak self] request in
            guard let self else {
                return .failure(IPCErrorBody(code: "ui_shutting_down", message: "Peek.app is quitting", retryable: true))
            }
            return await self.handle(request)
        }
        if usesLivePanels { startLiveServices() }
        await link.start()
    }

    public func stop() async {
        for task in tasks { task.cancel() }
        tasks.removeAll()
        hotKeys?.unregisterAll()
        hotKeys = nil
        if let screenObserver { NotificationCenter.default.removeObserver(screenObserver) }
        screenObserver = nil
        for observer in keyWindowObservers { NotificationCenter.default.removeObserver(observer) }
        keyWindowObservers.removeAll()
        slotManager.releaseEscape()
        presenceMonitor?.stop()
        presenceMonitor = nil
        if let request = takeTelemetry() {
            do throws(DaemonLinkError) {
                _ = try await link.send(request, blobs: [], timeout: .seconds(2))
            } catch {
                logger.notice("dropped \(request.events.count) telemetry events at shutdown: \(error.description)")
            }
        }
        await link.stop()
    }

    private func installCallbacks() {
        let previousFinished = speech.onFinished
        speech.onFinished = { [weak self] finished in
            previousFinished?(finished)
            guard let self else { return }
            self.logger.info("speech done for \(finished.sendID): played \(finished.playedMs) of \(finished.totalMs) ms"
                + (finished.stoppedByUser ? ", stopped by the Carbon" : ""))
            self.sendSpeechDone(finished)
            self.slotManager.speechFinished(finished)
        }
        let previousFailed = speech.onFailed
        speech.onFailed = { [weak self] sendID, error in
            previousFailed?(sendID, error)
            guard let self else { return }
            self.logger.info("speech failed for \(sendID): \(error.description)")
            self.slotManager.speechFailed(sendID: sendID)
        }
        let previousBackdrop = backdrop.onChange
        backdrop.onChange = { [weak self] key, value in
            previousBackdrop?(key, value)
            self?.slotManager.backdropChanged(key, value)
        }
    }

    /// Hotkeys and screen observation exist only in the real app (never in tests).
    private func startLiveServices() {
        let hotKeys = HotKeyCenter()
        hotKeys.onPress = { [weak self] slot in self?.slotManager.summon(slot) }
        self.hotKeys = hotKeys
        registerHotKeys()
        screenObserver = NotificationCenter.default.addObserver(
            forName: NSApplication.didChangeScreenParametersNotification, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.slotManager.screensChanged()
                self?.record("display_changed", ["screens": .int(Int64(NSScreen.screens.count))], context: .production)
            }
        }
        // A key Peek window (typing, Settings, Simulation) takes Esc through AppKit: the Esc router lets go at once.
        for name in [NSWindow.didBecomeKeyNotification, NSWindow.didResignKeyNotification] {
            keyWindowObservers.append(NotificationCenter.default.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated { self?.slotManager.refreshEscape() }
            })
        }
        // Before link.start(): the first hello is followed by the real presence (the app may launch while locked).
        let monitor = PresenceMonitor { [weak self] presence in self?.presenceChanged(presence) }
        presenceMonitor = monitor
        monitor.start()
    }

    /// The screen was locked or unlocked, the displays slept or woke: tell peekd, in order (`presence`, with
    /// ``paused`` merged in).
    public func presenceChanged(_ presence: PresenceRequest) {
        let previous = carbonPresence
        carbonPresence = presence.with(paused: false)
        let state = presence.available ? "available" : "away (\(presence.reason.rawValue)); peekd holds new peeks until they can be seen"
        if presenceTask == nil {
            logger.info("presence at launch: \(state)")
        } else if carbonPresence != previous {
            logger.info("presence changed: \(state)")
        }
        sendPresence()
    }

    /// Sends the current screen state and pause to peekd, after any presence still on its way.
    private func sendPresence() {
        let request = carbonPresence.with(paused: paused)
        let link = self.link
        let before = presenceTask
        presenceTask = Task {
            await before?.value
            await link.setPresence(request)
        }
    }

    // MARK: Events from peekd

    /// Routes one peekd event. Public so Simulation and tests can feed events directly.
    public func handle(_ event: DaemonEvent) {
        switch event {
        case .slotsState(let state): updateSlots(state.slots)
        case .peekShow(let show): present(show)
        case .ttsBegin(let e): slotManager.routeTTS(.begin(e))
        case .ttsChunk(let e): slotManager.routeTTS(.chunk(e))
        case .ttsEnd(let e): slotManager.routeTTS(.end(e))
        case .ttsError(let e): slotManager.routeTTS(.failure(e))
        case .peekCancel(let e): cancel(sendID: e.sendID, reason: e.reason)
        case .sttResult(let result): applySTTResult(result)
        case .restarting(let e): logger.notice("peekd is restarting into build \(e.toBuild)")
        case .queueState(let e): slotManager.applyQueueState(e)
        case .unknown(let name, _): logger.info("ignoring unknown event \(name)")
        case .malformed(let name, let reason):
            lastProblem = "peekd sent a malformed \(name) event: \(reason)"
            logger.error("malformed \(name): \(reason)")
        }
    }

    /// Answers one peekd → UI request.
    public func handle(_ request: DaemonRequest) async -> DaemonReply {
        switch request {
        case .drawingValidate(let validate):
            let report = await drawing.validate(
                scriptAt: URL(fileURLWithPath: validate.scriptPath),
                options: ValidationOptions(preview: validate.preview, dumpFrame: validate.dumpFrame))
            return .validation(report)
        case .drawingLoad(let load):
            if let failure = await loadDrawing(key: load.siliconKey, sha256: load.sha256, path: load.scriptPath) {
                return .failure(IPCErrorBody(code: "drawing_load_failed", message: failure.message, retryable: false))
            }
            return .encode(DrawingLoadResult(ok: true))
        case .appUpdatePrepare:
            return .encode(ReadyResult(ready: isIdleForUpdate))
        case .appQuit:
            let ready = isIdleForUpdate
            if ready {
                Task { @MainActor in
                    try? await Task.sleep(for: .milliseconds(200))
                    NSApp?.terminate(nil)
                }
            }
            return .encode(ReadyResult(ready: ready))
        case .appUninstall:
            guard let uninstallHandler else {
                return .failure(IPCErrorBody(
                    code: "uninstall_unavailable",
                    message: "this Peek.app cannot uninstall itself here (Simulation or test presenter)",
                    hint: "quit Peek, then move ~/Applications/Peek.app to the Trash"))
            }
            logger.notice("peekd relayed app.uninstall: uninstalling Peek.app")
            Task { @MainActor in
                // Let the reply reach peekd before the bundle moves and the app quits.
                try? await Task.sleep(for: .milliseconds(250))
                await uninstallHandler()
            }
            return .encode(UninstallAccepted(
                accepted: true, bundlePath: Bundle.main.bundleURL.path, unregistersServices: servicesEnabled))
        case .uiStatus:
            let report = statusReport()
            lastSentStatus = report
            return .encode(report)
        case .unknown(let op, _):
            return .failure(.unknownOp(op))
        }
    }

    // MARK: ui.status (peekd's doctor op)

    /// What only Peek.app knows, for `peek doctor` (mic permission, hotkeys, glass, services).
    public func statusReport() -> UIStatusReport {
        let info = Bundle.main.infoDictionary
        let build = (info?["CFBundleVersion"] as? String).flatMap(Int.init) ?? 0
        let version = info?["CFBundleShortVersionString"] as? String ?? "0.0.0"
        return UIStatusReport(
            mic: mic.permission.rawValue,
            hotkeys: .init(modifier: settings.hotkeyModifier.rawValue, registered: registeredHotkeys, failed: failedHotkeys,
                           problems: hotkeyProblems),
            glass: glassMode.rawValue, services: servicesEnabled ? "enabled" : "disabled", appBuild: build,
            appVersion: version)
    }

    /// Sends `ui.status` when it differs from what peekd last got. Only the app's own coordinator reports
    /// (Simulation's presenter talks to a fake link and has no hotkeys).
    func pushStatusIfChanged() {
        guard settingsURL != nil, linkState.isConnected, !statusUnsupported else { return }
        let report = statusReport()
        guard report != lastSentStatus else { return }
        lastSentStatus = report
        let link = self.link
        Task { [weak self] in
            do throws(DaemonLinkError) {
                _ = try await link.send(report)
            } catch {
                guard let self else { return }
                if case .remote(_, let body) = error, body.code == "unknown_op" {
                    // An older peekd: `peek doctor` degrades to "not reported"; do not retry until the next hello.
                    self.statusUnsupported = true
                    self.logger.info("peekd does not take ui.status yet (older build); peek doctor will not show mic/hotkeys")
                } else {
                    self.lastSentStatus = nil
                    self.logger.notice("ui.status failed: \(error.description)")
                }
            }
        }
    }

    // MARK: PeekPresenting

    public func updateSlots(_ newSlots: [SlotState]) {
        let sorted = newSlots.sorted { ($0.index, $0.context.rawValue) < ($1.index, $1.context.rawValue) }
        let newKeys = Set(sorted.map(\.siliconKey))
        for gone in Set(slots.map(\.siliconKey)).subtracting(newKeys) {
            // Unregistered: destroy the VM and forget the drawing (visual.md A1).
            hosts.removeValue(forKey: gone)?.unload()
            drawingLoads.removeValue(forKey: gone)?.task.cancel()
            input.remove(gone)
            slotManager.forget(gone)
        }
        slots = sorted
        slotManager.updateTable(sorted)
        for state in sorted {
            let host = host(for: state.siliconKey)
            if let ref = state.drawing, host.status != .ready(sha256: ref.sha256) {
                let key = state.siliconKey
                Task { [weak self] in _ = await self?.loadDrawing(key: key, sha256: ref.sha256, path: ref.path) }
            }
        }
        registerHotKeys()
        if !reportedGlassMode, sorted.contains(where: { $0.context != .simulation }) {
            reportedGlassMode = true
            record("glass_mode", ["mode": .string(glassMode.rawValue)], context: .production)
        }
    }

    public func present(_ show: PeekShowEvent) {
        slotManager.present(show)
    }

    public func cancel(sendID: String, reason: PeekCancelReason) {
        logger.info("cancel \(sendID): \(reason.rawValue)")
        slotManager.cancel(sendID: sendID)
    }

    public func applySTTResult(_ result: STTResultEvent) {
        slotManager.applySTT(result)
    }

    public var isIdleForUpdate: Bool { slotManager.isIdle }

    public func dismissAll() {
        slotManager.dismissAll()
    }

    /// Brings a slot forward as if its hotkey was pressed (menu bar, tests).
    public func summon(_ slot: SlotIndex) {
        slotManager.summon(slot)
    }

    // MARK: Settings

    /// Applies one setting, persists settings.json and tells peekd (`settings.changed`).
    public func setSetting(_ key: PeekSettings.Key, _ value: JSONValue) {
        do throws(SettingsError) {
            try settings.apply(key, value)
        } catch {
            lastProblem = error.description
            return
        }
        switch key {
        case .backdrop: backdrop.source = settings.backdrop
        case .mode: slotManager.setMode(settings.mode)
        case .display: slotManager.setDisplay(settings.display)
        case .hotkeyModifier: registerHotKeys()
        case .showTestPeeks: applyGate()
        case .telemetry: if !settings.telemetry { telemetryBuffer.removeAll() }
        case .voiceDefaults, .sttLanguage, .cliWatchdog: break
        }
        if let settingsURL {
            do {
                try settings.write(to: settingsURL)
            } catch {
                lastProblem = "cannot save settings to \(settingsURL.path): \(error)"
            }
        }
        // Only the app's own coordinator owns settings.json; Simulation's presenter changes a private copy.
        if settingsURL != nil { record("settings_changed", ["key": .string(key.rawValue)], context: .production) }
        sendInBackground(SettingsChangedRequest(key: key, value: settings.value(for: key)))
    }

    private func applyGate() {
        slotManager?.setGate(SlotGate(paused: paused, showTestPeeks: settings.showTestPeeks))
    }

    private func registerHotKeys() {
        guard let hotKeys else { return }
        let wanted = HotKeyPlan.slots(for: slots)
        let problems = hotKeys.update(slots: wanted, modifier: settings.hotkeyModifier)
        let modifier = settings.hotkeyModifier
        registeredHotkeys = hotKeys.registered.sorted().map { modifier.label(for: $0) }
        failedHotkeys = wanted.subtracting(hotKeys.registered).sorted().map { modifier.label(for: $0) }
        if let first = problems.first { lastProblem = first }
        updateHotkeyProblems()
    }

    /// The per-position shortcuts' problems plus Esc's (bare Esc taken by another app), for Settings and `ui.status`.
    private func updateHotkeyProblems() {
        var problems = hotKeys?.problems ?? []
        if let escape = slotManager.escapeProblem {
            problems.append(escape)
            lastProblem = escape
        }
        if problems != hotkeyProblems { hotkeyProblems = problems }
        pushStatusIfChanged()
    }

    // MARK: Drawings

    /// The drawing host for a Silicon, created on first use.
    public func host(for key: SiliconKey) -> any DrawingHosting {
        if let host = hosts[key] { return host }
        let initial = slots.first { $0.siliconKey == key }?.initial
            ?? SlotState(index: .top, context: key.context, actorID: key.actorID.isEmpty ? "?" : key.actorID,
                         orgID: key.orgID).initial
        let host = drawing.makeHost(for: key, initial: initial, images: images)
        host.input = input.source(for: key)
        let previousFailure = host.onFailure
        host.onFailure = { [weak self] failure in
            previousFailure?(failure)
            self?.drawingFailed(key, failure)
        }
        let previousLog = host.onLog
        host.onLog = { [weak self] line in
            previousLog?(line)
            guard let self, key.context == .simulation else { return }
            self.drawingLog.append(line)
            if self.drawingLog.count > 200 { self.drawingLog.removeFirst(self.drawingLog.count - 200) }
        }
        hosts[key] = host
        return host
    }

    /// Loads `path` into `key`'s host unless that exact script is already loaded or loading.
    /// Returns the failure, or nil when the drawing is active.
    func loadDrawing(key: SiliconKey, sha256: String, path: String) async -> DrawingFailure? {
        let host = host(for: key)
        if host.status == .ready(sha256: sha256) { return nil }
        if let inFlight = drawingLoads[key], inFlight.sha256 == sha256 { return await inFlight.task.value }
        drawingLoads[key]?.task.cancel()
        let task = Task<DrawingFailure?, Never> { @MainActor in
            let url = URL(fileURLWithPath: path)
            let data: Data
            do {
                data = try Data(contentsOf: url)
            } catch {
                return DrawingFailure(
                    reason: .throwsRepeatedly,
                    message: "Peek.app cannot read the drawing at \(path): \(error.localizedDescription)")
            }
            do throws(DrawingFailure) {
                try await host.load(DrawingScript(key: key, sha256: sha256, source: data, filename: url.lastPathComponent))
                return nil
            } catch {
                return error
            }
        }
        drawingLoads[key] = (sha256, task)
        let failure = await task.value
        if drawingLoads[key]?.sha256 == sha256 { drawingLoads.removeValue(forKey: key) }
        if let failure { logger.error("loading the drawing for \(key) failed: \(failure.message)") }
        return failure
    }

    private func drawingFailed(_ key: SiliconKey, _ failure: DrawingFailure) {
        record("fallback_visual", ["reason": .string(failure.reason.rawValue)], context: key.context)
        logger.info("drawing error for \(key): \(failure.reason.rawValue): \(failure.message); showing the fallback visual")
        guard !key.actorID.isEmpty else { return }
        sendInBackground(DrawingErrorRequest(context: key.context, orgID: key.orgID, actorID: key.actorID,
                                             reason: failure.reason, message: failure.message, stack: failure.stack))
    }

    // MARK: Requests to peekd

    /// Builds and sends a bubble's request. Returns whether peekd took it (with the message id of a voice message),
    /// or what went wrong as a notice for the Carbon.
    func send(_ outbound: BubbleOutbound, context: BubbleContext) async -> BubbleDelivery {
        let wireContext: PeekContext? = context.context == .production ? nil : context.context
        do throws(DaemonLinkError) {
            switch outbound {
            case .answer(let value, let via):
                guard let sendID = context.sendID, let askID = context.askID else {
                    return .failed("this bubble has no ask to answer")
                }
                _ = try await link.send(AnswerRequest(sendID: sendID, askID: askID, value: value, via: via))
            case .dismissed(let gesture):
                guard let sendID = context.sendID else { return .delivered }
                _ = try await link.send(DismissedRequest(sendID: sendID, gesture: gesture))
            case .shownDone(let reason, let visibleMs):
                guard let sendID = context.sendID else { return .delivered }
                _ = try await link.send(ShownDoneRequest(sendID: sendID, visibleMs: visibleMs, reason: reason))
            case .message(let text):
                _ = try await link.send(MessageRequest(slot: context.slot, text: text, context: wireContext))
            case .voice(let recording):
                let request = VoiceSubmitRequest(
                    sendID: context.sendID, askID: context.askID, slot: context.slot, durationMs: recording.durationMs,
                    context: wireContext, languages: sttLanguages())
                let reply = try await link.send(request, blobs: [recording.wav], timeout: nil)
                // peekd names a voice message so its stt.result is matched by id, not by arrival order.
                return BubbleDelivery(messageID: context.askID == nil ? reply.messageID : nil)
            case .focus:
                _ = try await link.send(FocusRequest(slot: context.slot, context: wireContext))
            case .shown:
                guard let sendID = context.sendID, !shownUnsupported else { return .delivered }
                _ = try await link.send(ShownRequest(sendID: sendID))
            }
            return .delivered
        } catch {
            if case .shown = outbound, case .remote(_, let body) = error, body.code == "unknown_op" {
                // An older peekd (an update swap in progress) sets shown_at when it pushes: nothing is lost.
                shownUnsupported = true
                logger.info("peekd does not take shown yet (older build); it counts a peek as shown when it pushes it")
                return .delivered
            }
            logger.error("sending \(String(describing: outbound).prefix(40)) failed: \(error.description)")
            return .failed(Self.notice(for: error))
        }
    }

    /// A short sentence for the bubble when peekd did not take a request.
    static func notice(for error: DaemonLinkError) -> String {
        switch error {
        case .notConnected, .disconnected:
            "Not sent: Peek's helper (peekd) isn't reachable. Try again in a moment."
        case .timedOut:
            "Not sent: peekd didn't answer in time. Try again."
        case .remote(_, let body):
            "Not sent: \(body.message)"
        case .invalidReply, .invalidRequest:
            "Not sent: \(error.description)"
        }
    }

    private func sttLanguages() -> [String] {
        settings.sttLanguage == "auto" ? Array(Locale.preferredLanguages.prefix(4)) : [settings.sttLanguage]
    }

    private func sendSpeechDone(_ finished: SpeechFinished) {
        guard speechDoneSent.insert(finished.sendID).inserted else { return }
        if speechDoneSent.count > 512 { speechDoneSent = [finished.sendID] }
        sendInBackground(finished.request)
    }

    private func sendInBackground<R: UIRequest>(_ request: R) {
        let link = self.link
        Task { [weak self] in
            do throws(DaemonLinkError) {
                _ = try await link.send(request)
            } catch {
                self?.logger.error("\(R.op) failed: \(error.description)")
            }
        }
    }

    // MARK: Telemetry (BLUEPRINT §6.5 "mac"; relayed by peekd; opt-out in Settings)

    private func record(_ type: String, _ data: [String: JSONValue], context: PeekContext) {
        guard settings.telemetry, context != .simulation else { return }
        telemetryBuffer.append(TelemetryEvent(type: type, data: .object(data), metadata: ["source": "peek.app"]))
        if telemetryBuffer.count >= Self.telemetryBatch { flushTelemetry() }
    }

    private func flushTelemetry() {
        if let request = takeTelemetry() { sendInBackground(request) }
    }

    private func takeTelemetry() -> TelemetryRequest? {
        defer { telemetryBuffer.removeAll() }
        guard !telemetryBuffer.isEmpty, settings.telemetry else { return nil }
        return TelemetryRequest(events: telemetryBuffer)
    }
}

/// Builds the live object graph for the app.
@MainActor
public enum PeekComposition {
    public static func makeLive(bundle: Bundle = .main) -> PeekCoordinator {
        let paths = PeekPaths.current
        let (settings, warnings) = PeekSettings.load(from: paths.settingsFile)
        let images = ImageCache(paths: paths)
        let speech = SpeechPlayer()
        let mic = MicRecorder()
        let backdrop = BackdropSampler(paths: paths)
        return PeekCoordinator(
            link: DaemonLink(configuration: .live(bundle: bundle)), drawing: DrawingRuntime(), speech: speech, mic: mic,
            images: images, input: InputHub(images: images, speech: speech, mic: mic, backdrop: backdrop),
            backdrop: backdrop, paths: paths, settings: settings, settingsWarnings: warnings, presentation: .live)
    }
}
