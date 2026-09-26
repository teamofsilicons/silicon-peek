import AppKit
import OSLog
import PeekCore
import QuartzCore

/// One Silicon's drawing (visual.md B1 `DrawingHost`): a QuickJS VM on its own thread, the frame scheduler,
/// and the compositor that turns frames into layer views inside PeekUI's visual view.
///
/// Frame loop (B4): on a display tick, if awake and no frame is in flight, the host builds `input` from its
/// ``DrawingInputSource`` and posts it to the VM thread; there `frame()` runs under a 4 ms deadline and the
/// result is decoded and rasterised; back on the main thread the compositor applies it. `again == false`
/// puts the drawing to sleep until a wake reason (A3) or an event arrives; a speech or mic level above 0
/// keeps it awake. `input.dt` is the time since the last executed frame, capped at 0.1 s, 0 after waking.
///
/// Failures (B10): a throwing frame is dropped (10 in a row → fallback), an overrun is dropped (30 within
/// 5 s → fallback), an out-of-memory VM is destroyed (→ fallback). The fallback is reported once through
/// ``onFailure`` so PeekUI can send `drawing.error`.
@MainActor
public final class DrawingHost: DrawingHosting {
    public let key: SiliconKey
    public private(set) var status: DrawingHostStatus = .empty
    public var input: (any DrawingInputSource)? {
        didSet {
            oldValue?.onWake = nil
            input?.onWake = { [weak self] _ in self?.wake() }
        }
    }
    public var onFailure: (@MainActor (DrawingFailure) -> Void)?
    public var onLog: (@MainActor (String) -> Void)?

    public let initial: String
    public let glassMode: GlassMode
    private let images: any ImageProviding
    let canvas = DrawingCanvasView(frame: NSRect(x: 0, y: 0, width: 100, height: 100))
    let compositor: Compositor
    let scheduler = FrameScheduler()
    private var worker: DrawingWorker?
    private var generation = 0
    private static let logger = PeekLogger(category: "drawing")

    // Frame loop state.
    private(set) var isAwake = false
    private(set) var isFrameInFlight = false
    private var wakePending = false
    private var firstFrameAfterWake = true
    private var hasRenderedSinceLoad = false
    private var loadTime: CFTimeInterval = 0
    private var lastFrameTime: CFTimeInterval?
    private var consecutiveThrows = 0
    private var overrunTimes: [CFTimeInterval] = []
    private var reportedOpsCap = false
    /// The loaded script, for readable stacks (source line + caret) in logs and `drawing.error`.
    private var scriptSource: String?
    private var scriptFilename = "drawing.js"
    /// Frames completed since load (tests, diagnostics).
    private(set) var framesRendered = 0

    /// Monotonic seconds; injectable for tests.
    var clock: () -> CFTimeInterval = { CACurrentMediaTime() } {
        didSet { compositor.clock = clock }
    }
    /// When false, frames run only through ``tick(timestamp:)`` (tests drive the clock).
    let usesDisplayLink: Bool

    public init(key: SiliconKey, initial: String, images: any ImageProviding, glassMode: GlassMode = .live) {
        self.key = key
        self.initial = initial
        self.images = images
        self.glassMode = glassMode
        self.usesDisplayLink = true
        compositor = Compositor(canvas: canvas, glassMode: glassMode)
        scheduler.onTick = { [weak self] timestamp in self?.tick(timestamp: timestamp) }
        scheduler.onStall = { [weak self] in
            guard let self else { return }
            Self.logger.info("drawing \(self.key): the display link is not firing (display asleep or off screen); "
                + "running frames from a 10 Hz timer so errors are still caught")
        }
    }

    /// A host without a display link, for tests.
    init(key: SiliconKey, initial: String, images: any ImageProviding, glassMode: GlassMode, manualClock: Bool) {
        self.key = key
        self.initial = initial
        self.images = images
        self.glassMode = glassMode
        self.usesDisplayLink = !manualClock
        compositor = Compositor(canvas: canvas, glassMode: glassMode)
        scheduler.onTick = { [weak self] timestamp in self?.tick(timestamp: timestamp) }
        scheduler.onStall = { [weak self] in
            guard let self else { return }
            Self.logger.info("drawing \(self.key): the display link is not firing (display asleep or off screen); "
                + "running frames from a 10 Hz timer so errors are still caught")
        }
    }

    isolated deinit {
        worker?.shutdown()
        scheduler.detach()
    }

    // MARK: Loading

    public func load(_ script: DrawingScript) async throws(DrawingFailure) {
        stopWorker()
        generation += 1
        let loadGeneration = generation
        compositor.clear()
        compositor.hideFallback()
        status = .loading
        resetFrameState()
        scriptSource = String(validating: script.source, as: UTF8.self)
        scriptFilename = script.filename

        guard script.source.count <= DrawingLimits.maxScriptBytes else {
            let failure = DrawingFailure(
                reason: .throwsRepeatedly,
                message: String(format: "%@ is %.1f KB; drawings are limited to 256 KB (%@)",
                                script.filename, Double(script.source.count) / 1024, DrawingDocs.limits))
            enterFallback(failure, report: false)
            throw failure
        }
        let worker = DrawingWorker(name: "ai.tos.peek.drawing.\(key.actorID)") { [weak self] line in
            DispatchQueue.main.async {
                MainActor.assumeIsolated { self?.onLog?(line) }
            }
        }
        self.worker = worker
        let result = await worker.load(source: Array(script.source), filename: script.filename)
        guard loadGeneration == generation else {
            throw DrawingFailure(reason: .throwsRepeatedly,
                                 message: "loading \(script.filename) was superseded by another load or unload")
        }
        let failure: DrawingFailure
        switch result {
        case .ok:
            status = .ready(sha256: script.sha256)
            loadTime = clock()
            // Already on screen (a re-registered drawing): show its first frame instead of a blank square.
            if canvas.superview != nil { wake() }
            return
        case .threw(let message, let stack):
            failure = DrawingFailure(reason: .throwsRepeatedly, message: "loading \(script.filename) threw: \(message)",
                                     stack: clean(stack))
        case .interrupted:
            failure = DrawingFailure(
                reason: .overruns,
                message: "the top-level code of \(script.filename) ran longer than 250 ms and was stopped")
        case .outOfMemory:
            failure = DrawingFailure(reason: .oom,
                                     message: "loading \(script.filename) exceeded the 16 MB memory limit (\(DrawingDocs.limits))")
        case .failed(let message):
            failure = DrawingFailure(reason: .throwsRepeatedly, message: "loading \(script.filename) failed: \(message)")
        }
        enterFallback(failure, report: false)
        throw failure
    }

    public func unload() {
        generation += 1
        stopWorker()
        compositor.clear()
        compositor.hideFallback()
        resetFrameState()
        status = .empty
    }

    private func stopWorker() {
        worker?.shutdown()
        worker = nil
        scheduler.stop()
    }

    private func resetFrameState() {
        isAwake = false
        isFrameInFlight = false
        wakePending = false
        firstFrameAfterWake = true
        hasRenderedSinceLoad = false
        lastFrameTime = nil
        consecutiveThrows = 0
        overrunTimes = []
        reportedOpsCap = false
        framesRendered = 0
    }

    // MARK: View

    public func attach(to visualView: NSView) {
        if canvas.superview !== visualView {
            canvas.removeFromSuperview()
            canvas.frame = visualView.bounds
            visualView.addSubview(canvas)
        }
        if usesDisplayLink { scheduler.attach(to: canvas) }
        if case .ready = status, !hasRenderedSinceLoad {
            wake()
        } else if isAwake {
            scheduler.start()
        }
    }

    public func detach() {
        scheduler.detach()
        canvas.removeFromSuperview()
    }

    // MARK: Events and waking

    public func deliver(_ event: DrawingEvent) {
        guard case .ready = status, let worker else { return }
        let deliveryGeneration = generation
        let name = event.name
        worker.event(name: name, payload: event.payloadJSON) { [weak self] status in
            guard !status.isOK else { return }
            DispatchQueue.main.async {
                MainActor.assumeIsolated { self?.eventFailed(name: name, status: status, generation: deliveryGeneration) }
            }
        }
        wake()
    }

    private func eventFailed(name: String, status: EngineStatus, generation: Int) {
        guard generation == self.generation else { return }
        switch status {
        case .outOfMemory(let message):
            enterFallback(DrawingFailure(reason: .oom, message: "the '\(name)' handler exceeded the 16 MB memory limit: "
                                             + message), report: true)
        case .threw(let message, let stack):
            report("the '\(name)' handler threw: \(message)" + (clean(stack).map { "\n\($0)" } ?? ""))
        case .interrupted:
            report("the '\(name)' handler ran longer than 4 ms and was stopped")
        case .failed(let message):
            report("delivering '\(name)' failed: \(message)")
        case .ok:
            break
        }
    }

    public func wake() {
        guard case .ready = status else { return }
        if isFrameInFlight {
            wakePending = true
        }
        if !isAwake {
            isAwake = true
            firstFrameAfterWake = true
        }
        scheduler.start()
    }

    // MARK: Frames

    /// One display tick: the scheduler calls it; tests drive it directly with their own timestamps.
    func tick(timestamp now: CFTimeInterval) {
        guard case .ready = status, isAwake, !isFrameInFlight, let worker else { return }
        let dt = firstFrameAfterWake ? 0 : min(DrawingLimits.maxDeltaTime, max(0, now - (lastFrameTime ?? now)))
        firstFrameAfterWake = false
        lastFrameTime = now
        let snapshot = input?.snapshot(t: max(0, now - loadTime), dt: dt) ?? defaultSnapshot(t: now - loadTime, dt: dt)
        let frameImages = resolveImages(in: snapshot)
        isFrameInFlight = true
        wakePending = false
        let frameGeneration = generation
        let keepAwakeForLevels = (snapshot.speech?.level ?? 0) > 0 || snapshot.mic.level > 0
        worker.frame(input: snapshot.jsonBytes(), images: frameImages, pixels: pixelSize) { [weak self] outcome in
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    self?.finishFrame(outcome, generation: frameGeneration, levelsAwake: keepAwakeForLevels, at: now)
                }
            }
        }
    }

    private func finishFrame(_ outcome: FrameOutcome, generation frameGeneration: Int, levelsAwake: Bool,
                             at time: CFTimeInterval) {
        guard frameGeneration == generation else { return }
        isFrameInFlight = false
        var keepAwake = true
        switch outcome.status {
        case .ok:
            consecutiveThrows = 0
            if let rendered = outcome.rendered {
                compositor.apply(rendered)
                hasRenderedSinceLoad = true
                framesRendered += 1
                for message in rendered.diagnostics { report(message) }
                if rendered.droppedOps > 0, !reportedOpsCap {
                    reportedOpsCap = true
                    report("a frame recorded more than \(DrawingLimits.maxOpsPerFrame) ops; \(rendered.droppedOps) were "
                        + "ignored (\(DrawingDocs.limits))")
                }
                keepAwake = rendered.again || wakePending || levelsAwake
            }
        case .threw(let message, let stack):
            consecutiveThrows += 1
            let stack = clean(stack)
            report("frame() threw: \(message)" + (stack.map { "\n\($0)" } ?? ""))
            if consecutiveThrows >= DrawingLimits.maxConsecutiveThrows {
                enterFallback(DrawingFailure(
                    reason: .throwsRepeatedly,
                    message: "frame() threw \(consecutiveThrows) times in a row; last error: \(message)", stack: stack),
                    report: true)
                return
            }
        case .interrupted:
            overrunTimes.append(time)
            overrunTimes.removeAll { time - $0 > DrawingLimits.overrunWindow }
            if overrunTimes.count >= DrawingLimits.maxOverruns {
                enterFallback(DrawingFailure(
                    reason: .overruns,
                    message: "frame() ran longer than 4 ms \(overrunTimes.count) times within 5 s (\(DrawingDocs.limits))"),
                    report: true)
                return
            }
        case .outOfMemory(let message):
            enterFallback(DrawingFailure(reason: .oom, message: "the drawing exceeded the 16 MB memory limit: \(message)"),
                          report: true)
            return
        case .failed(let message):
            consecutiveThrows += 1
            report("frame() failed: \(message)")
        }
        if keepAwake {
            scheduler.start()
        } else {
            isAwake = false
            scheduler.stop()
        }
    }

    private func enterFallback(_ failure: DrawingFailure, report shouldReport: Bool) {
        stopWorker()
        isAwake = false
        isFrameInFlight = false
        status = .fallback(failure)
        compositor.showFallback(initial: initial)
        let who = key.description, message = failure.message
        Self.logger.error("drawing \(who) switched to the fallback visual: \(message)")
        if shouldReport { onFailure?(failure) }
    }

    private func clean(_ stack: String?) -> String? {
        DrawingStack.clean(stack, source: scriptSource, filename: scriptFilename)
    }

    private func report(_ message: String) {
        let who = key.description
        Self.logger.notice("drawing \(who): \(message)")
        onLog?(message)
    }

    /// Image handles referenced by this frame's input, resolved to decoded images (visual.md B7).
    private func resolveImages(in snapshot: InputSnapshot) -> [Int: CGImage] {
        var handles: [ImageHandle] = []
        for element in snapshot.show?.elements ?? [] {
            if case .image(let handle, _, _) = element { handles.append(handle) }
        }
        for option in snapshot.ask?.options ?? [] {
            if let handle = option.image { handles.append(handle) }
        }
        var resolved: [Int: CGImage] = [:]
        for handle in handles {
            if let image = images.image(for: handle) { resolved[handle.id] = image }
        }
        return resolved
    }

    private func defaultSnapshot(t: Double, dt: Double) -> InputSnapshot {
        InputSnapshot(t: max(0, t), dt: dt, slot: .init(index: .top, facing: .pi / 2), glass: glassMode)
    }

    /// Bitmap size of the draw layers: the canvas in points × the backing scale.
    var pixelSize: Int {
        let points = canvas.bounds.width > 0 ? canvas.bounds.width : 120
        let scale = canvas.window?.backingScaleFactor ?? NSScreen.main?.backingScaleFactor ?? 2
        return min(1024, max(16, Int((points * scale).rounded())))
    }

    // MARK: Hit testing and validation

    public func isOverContent(unitPoint: CGPoint) -> Bool {
        if case .fallback = status { return FallbackHitTest.contains(unitPoint) }
        return compositor.isOverContent(unitPoint: unitPoint)
    }

    public func validate(_ script: DrawingScript, options: ValidationOptions) async -> ValidationReport {
        await DrawingValidator.validate(script, options: options, glassMode: glassMode)
    }
}
