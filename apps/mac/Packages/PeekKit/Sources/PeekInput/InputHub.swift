import AppKit
import Foundation
import PeekCore

/// Builds each drawing's per-frame `input` (visual.md A4 as amended by BLUEPRINT §0.1, B8) and publishes the
/// changes that wake a sleeping drawing (A3, without "a word is spoken").
///
/// | Input | Source |
/// |---|---|
/// | `slot`, `mode`, `phase`, `context`, `glass`, `typing` | ``BubbleInputState`` from PeekUI |
/// | `appearance` | ``AppearanceProviding`` (KVO on `NSApp.effectiveAppearance`) |
/// | `backdrop` | ``BackdropSampling``; InputHub tracks each visible bubble's visual square |
/// | `hover`, `mouse` | ``PointerTracking`` (`NSEvent.mouseLocation` each frame, global/local move monitors) |
/// | `speech` | ``SpeechPlaying/playback(for:)`` for the bubble's send while it has speak text |
/// | `mic` | ``MicRecording/level`` while the bubble is listening |
/// | `show`, `ask` | the send's payload with image paths replaced by ``ImageProviding`` handles |
///
/// Wake reasons: phase, send content, ask value/highlight/typing (``WakeReason/answer``), slot or frame, mode,
/// appearance, backdrop, hover start/end and moves while hovering, and — polled at display rate only while some
/// bubble is speaking or listening — speech and mic levels above 0 (plus the frame they drop back to 0).
@MainActor
public final class InputHub: InputHubbing {
    /// Levels at or below this count as silent for waking.
    public static let audibleLevel = 0.001
    public static let audioPollInterval: Duration = .milliseconds(16)
    public static let backdropPollInterval: Duration = .milliseconds(500)

    public var appearance: Appearance { appearanceSource.current }

    private let images: any ImageProviding
    private let speech: any SpeechPlaying
    private let mic: any MicRecording
    private let backdrop: any BackdropSampling
    private let pointer: any PointerTracking
    private let appearanceSource: any AppearanceProviding

    private var states: [SiliconKey: BubbleInputState] = [:]
    private var sources: [SiliconKey: BubbleSource] = [:]
    private var extras: [SiliconKey: Extras] = [:]
    private var audioTask: Task<Void, Never>?
    private var backdropTask: Task<Void, Never>?
    private let backdropBroadcasts: Bool

    /// Per-bubble bookkeeping that is not part of ``BubbleInputState``.
    private struct Extras {
        var content: Content?
        var prepared: [String: PreparedImage] = [:]
        var preparedSendID: String?
        var requestedPaths: [String] = []
        var imageTask: Task<Void, Never>?
        var trackedRect: CGRect?
        var hover = false
        var speechLevel = 0.0
        var speechDone = false
        var micLevel = 0.0
        var backdrop: Backdrop?
    }

    private struct Content {
        var show: InputSnapshot.Show?
        var ask: InputSnapshot.Ask?
    }

    public init(images: any ImageProviding, speech: any SpeechPlaying, mic: any MicRecording,
                backdrop: any BackdropSampling, pointer: any PointerTracking = SystemPointer(),
                appearance: any AppearanceProviding = SystemAppearance()) {
        self.images = images
        self.speech = speech
        self.mic = mic
        self.backdrop = backdrop
        self.pointer = pointer
        appearanceSource = appearance
        if let sampler = backdrop as? BackdropSampler {
            backdropBroadcasts = true
            sampler.addObserver { [weak self] key, value in self?.backdropChanged(key, value) }
        } else {
            backdropBroadcasts = false
        }
        pointer.onMove = { [weak self] in self?.pointerMoved() }
        appearance.onChange = { [weak self] _ in self?.wakeAll(.appearanceChanged) }
    }

    isolated deinit {
        audioTask?.cancel()
        backdropTask?.cancel()
        pointer.stopMonitoring()
        for extra in extras.values { extra.imageTask?.cancel() }
    }

    // MARK: InputHubbing

    public func source(for key: SiliconKey) -> any DrawingInputSource {
        if let source = sources[key] { return source }
        let source = BubbleSource(key: key, hub: self)
        sources[key] = source
        return source
    }

    public func update(_ key: SiliconKey, _ mutate: (inout BubbleInputState) -> Void) {
        let before = states[key]
        var state = before ?? BubbleInputState(slot: .top)
        mutate(&state)
        guard state != before else { return }
        states[key] = state
        var extra = extras[key] ?? Extras()
        if before?.show != state.show || before?.ask != state.ask || before?.askValue != state.askValue
            || before?.askHighlight != state.askHighlight || before?.sendID != state.sendID
        {
            extra.content = nil
        }
        extras[key] = extra
        syncImages(key, state: state, previousSendID: before?.sendID)
        syncBackdropTracking(key, state: state)
        syncMonitoring()
        for reason in Self.wakeReasons(from: before, to: state) { sources[key]?.onWake?(reason) }
    }

    public func state(for key: SiliconKey) -> BubbleInputState? { states[key] }

    /// Forgets the bubble: releases its images and stops sampling its backdrop. The key's source object stays
    /// (sources are stable per key), so a drawing host that outlives the bubble keeps receiving wakes if the key
    /// comes back; until then it reads a hidden, empty input.
    public func remove(_ key: SiliconKey) {
        if let sendID = states[key]?.sendID { images.release(sendID: sendID) }
        extras[key]?.imageTask?.cancel()
        if extras[key]?.trackedRect != nil { backdrop.track(key, rectOnScreen: nil) }
        states.removeValue(forKey: key)
        extras.removeValue(forKey: key)
        syncMonitoring()
    }

    // MARK: Snapshot

    /// The input for one frame of `key`'s drawing.
    public func snapshot(for key: SiliconKey, t: Double, dt: Double) -> InputSnapshot {
        guard let state = states[key] else {
            return InputSnapshot(t: t, dt: dt, slot: .init(index: .top, facing: .pi / 2), appearance: appearance,
                                 backdrop: backdrop.backdrop(for: key))
        }
        let visible = state.phase != .hidden
        let mouse = visible
            ? PointerMath.mouse(fromScreen: pointer.location, visualFrameOnScreen: state.visualFrameOnScreen)
            : .outside
        let content = content(for: key, state: state)
        return InputSnapshot(
            t: t, dt: dt, slot: .init(index: state.slot, facing: state.facing), mode: state.mode,
            appearance: appearance, backdrop: backdrop.backdrop(for: key), phase: state.phase,
            hover: visible && mouse.inside, mouse: mouse, speech: speechInput(for: state),
            mic: .init(level: state.listening ? Self.clampLevel(mic.level) : 0),
            typing: state.typingText.map(InputSnapshot.Typing.init(text:)), show: content.show, ask: content.ask,
            context: state.context, glass: state.glass)
    }

    private func speechInput(for state: BubbleInputState) -> InputSnapshot.Speech? {
        guard let text = state.speakText else { return nil }
        guard let sendID = state.sendID, let playback = speech.playback(for: sendID) else {
            return InputSnapshot.Speech(text: text, level: 0, progress: 0, done: false)
        }
        return InputSnapshot.Speech(text: text, level: Self.clampLevel(playback.level),
                                    progress: min(max(playback.progress, 0), 1), done: playback.done)
    }

    private func content(for key: SiliconKey, state: BubbleInputState) -> Content {
        if let cached = extras[key]?.content { return cached }
        let prepared = extras[key].flatMap { $0.preparedSendID == state.sendID ? $0.prepared : nil } ?? [:]
        let show = state.show.map { payload in
            InputSnapshot.Show(elements: payload.elements.compactMap { element -> InputSnapshot.Show.Element? in
                switch element {
                case .text(let text):
                    return .text(text)
                case .image(let path, let caption):
                    // Drawn once decoded (a frame or two); an unreadable image is left out (B7).
                    guard let image = prepared[path] else { return nil }
                    return .image(image.handle, caption: caption, colors: image.colors)
                }
            })
        }
        let ask = state.ask.map {
            InputSnapshot.Ask($0, value: state.askValue, highlight: state.askHighlight, images: prepared)
        }
        let content = Content(show: show, ask: ask)
        extras[key]?.content = content
        return content
    }

    // MARK: Images

    private func syncImages(_ key: SiliconKey, state: BubbleInputState, previousSendID: String?) {
        guard var extra = extras[key] else { return }
        if let previousSendID, previousSendID != state.sendID {
            // The next send replaces the content: the old handles become invalid (B7).
            extra.imageTask?.cancel()
            extra.imageTask = nil
            extra.prepared = [:]
            extra.preparedSendID = nil
            extra.requestedPaths = []
            images.release(sendID: previousSendID)
        }
        let paths = Self.imagePaths(of: state)
        guard let sendID = state.sendID, !paths.isEmpty else {
            extras[key] = extra
            return
        }
        guard paths != extra.requestedPaths || extra.preparedSendID != sendID && extra.imageTask == nil else {
            extras[key] = extra
            return
        }
        extra.requestedPaths = paths
        extra.imageTask?.cancel()
        let images = self.images
        extra.imageTask = Task { [weak self] in
            let prepared = await images.prepare(sendID: sendID, paths: paths)
            guard !Task.isCancelled, let self, self.states[key]?.sendID == sendID else { return }
            self.extras[key]?.prepared = prepared
            self.extras[key]?.preparedSendID = sendID
            self.extras[key]?.content = nil
            self.extras[key]?.imageTask = nil
            self.sources[key]?.onWake?(.send)
        }
        extras[key] = extra
    }

    static func imagePaths(of state: BubbleInputState) -> [String] {
        var seen = Set<String>()
        return ((state.show?.imagePaths ?? []) + (state.ask?.imagePaths ?? [])).filter { seen.insert($0).inserted }
    }

    // MARK: Backdrop

    private func syncBackdropTracking(_ key: SiliconKey, state: BubbleInputState) {
        let frame = state.visualFrameOnScreen
        let want: CGRect? = state.phase != .hidden && frame.width > 0 && frame.height > 0 ? frame : nil
        guard extras[key]?.trackedRect != want else { return }
        extras[key]?.trackedRect = want
        backdrop.track(key, rectOnScreen: want)
        extras[key]?.backdrop = backdrop.backdrop(for: key)
    }

    private func backdropChanged(_ key: SiliconKey, _ value: Backdrop) {
        guard states[key] != nil else { return }
        extras[key]?.backdrop = value
        sources[key]?.onWake?(.backdropChanged)
    }

    /// For backdrop implementations that cannot notify InputHub directly (e.g. Simulation's): compares values.
    func pollBackdrops() {
        for key in visibleKeys() {
            let value = backdrop.backdrop(for: key)
            guard let previous = extras[key]?.backdrop else {
                extras[key]?.backdrop = value
                continue
            }
            guard previous.tone != value.tone || previous.color != value.color || previous.source != value.source
            else { continue }
            extras[key]?.backdrop = value
            sources[key]?.onWake?(.backdropChanged)
        }
    }

    // MARK: Pointer

    func pointerMoved() {
        let location = pointer.location
        for key in visibleKeys() {
            guard let state = states[key] else { continue }
            let hover = PointerMath.mouse(fromScreen: location, visualFrameOnScreen: state.visualFrameOnScreen).inside
            let was = extras[key]?.hover ?? false
            extras[key]?.hover = hover
            if hover != was {
                sources[key]?.onWake?(.hover)
            } else if hover {
                sources[key]?.onWake?(.pointerMoved)
            }
        }
    }

    // MARK: Audio

    /// Wakes drawings while speech or the mic is audible (A3: "`input.speech.level` or `input.mic.level` goes
    /// above 0"), once more when a level falls back to 0, and when speech finishes.
    func pollAudio() {
        for key in visibleKeys() {
            guard let state = states[key], var extra = extras[key] else { continue }
            if state.speakText != nil, let sendID = state.sendID {
                let playback = speech.playback(for: sendID)
                let level = Self.clampLevel(playback?.level ?? 0)
                let done = playback?.done ?? false
                let audible = level > Self.audibleLevel
                let wasAudible = extra.speechLevel > Self.audibleLevel
                extra.speechLevel = level
                if audible || wasAudible || done != extra.speechDone { sources[key]?.onWake?(.speechLevel) }
                extra.speechDone = done
            }
            let micLevel = state.listening ? Self.clampLevel(mic.level) : 0
            if micLevel > Self.audibleLevel || extra.micLevel > Self.audibleLevel { sources[key]?.onWake?(.micLevel) }
            extra.micLevel = micLevel
            extras[key] = extra
        }
        syncMonitoring()
    }

    private func needsAudioPolling() -> Bool {
        visibleKeys().contains { key in
            guard let state = states[key] else { return false }
            if state.listening { return true }
            if (extras[key]?.micLevel ?? 0) > Self.audibleLevel { return true }
            guard state.speakText != nil, state.sendID != nil else { return false }
            return !(extras[key]?.speechDone ?? false) || (extras[key]?.speechLevel ?? 0) > Self.audibleLevel
        }
    }

    // MARK: Monitoring

    private func visibleKeys() -> [SiliconKey] {
        states.filter { $0.value.phase != .hidden }.map(\.key).sorted { $0.description < $1.description }
    }

    /// Starts or stops the pointer monitors and the polling loops for the current bubbles.
    private func syncMonitoring() {
        let anyVisible = states.values.contains { $0.phase != .hidden }
        if anyVisible, !pointer.isMonitoring {
            pointer.startMonitoring()
        } else if !anyVisible, pointer.isMonitoring {
            pointer.stopMonitoring()
        }
        if needsAudioPolling() {
            if audioTask == nil { audioTask = loop(every: Self.audioPollInterval) { $0.pollAudio() } }
        } else {
            audioTask?.cancel()
            audioTask = nil
        }
        if anyVisible, !backdropBroadcasts {
            if backdropTask == nil { backdropTask = loop(every: Self.backdropPollInterval) { $0.pollBackdrops() } }
        } else {
            backdropTask?.cancel()
            backdropTask = nil
        }
    }

    /// Whether the display-rate audio loop runs (for tests).
    var isPollingAudio: Bool { audioTask != nil }

    private func loop(every interval: Duration, _ body: @escaping @MainActor (InputHub) -> Void) -> Task<Void, Never> {
        Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: interval)
                guard !Task.isCancelled, let self else { return }
                body(self)
            }
        }
    }

    private func wakeAll(_ reason: WakeReason) {
        for key in sources.keys.sorted(by: { $0.description < $1.description }) { sources[key]?.onWake?(reason) }
    }

    // MARK: Wake reasons

    /// What changed between two states, as wake reasons in a fixed order (each at most once).
    static func wakeReasons(from before: BubbleInputState?, to after: BubbleInputState) -> [WakeReason] {
        let old = before ?? BubbleInputState(slot: after.slot)
        var reasons: [WakeReason] = []
        if old.phase != after.phase { reasons.append(.phaseChanged) }
        if old.sendID != after.sendID || old.speakText != after.speakText || old.show != after.show
            || old.ask != after.ask
        {
            reasons.append(.send)
        }
        if old.askValue != after.askValue || old.askHighlight != after.askHighlight || old.typingText != after.typingText {
            reasons.append(.answer)
        }
        if old.listening != after.listening { reasons.append(.micLevel) }
        if before == nil || old.slot != after.slot || old.facing != after.facing
            || old.visualFrameOnScreen != after.visualFrameOnScreen
        {
            reasons.append(.slotChanged)
        }
        if old.mode != after.mode || old.glass != after.glass || old.context != after.context {
            reasons.append(.modeChanged)
        }
        return reasons
    }

    static func clampLevel(_ level: Double) -> Double {
        level.isFinite ? min(max(level, 0), 1) : 0
    }
}

/// One drawing's view of the hub; stable per key.
@MainActor
final class BubbleSource: DrawingInputSource {
    let key: SiliconKey
    weak var hub: InputHub?
    var onWake: (@MainActor (WakeReason) -> Void)?

    init(key: SiliconKey, hub: InputHub) {
        self.key = key
        self.hub = hub
    }

    func snapshot(t: Double, dt: Double) -> InputSnapshot {
        hub?.snapshot(for: key, t: t, dt: dt) ?? InputSnapshot(t: t, dt: dt, slot: .init(index: .top, facing: .pi / 2))
    }
}
