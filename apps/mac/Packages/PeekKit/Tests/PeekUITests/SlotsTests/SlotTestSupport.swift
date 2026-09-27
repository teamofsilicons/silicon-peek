import AppKit
import Foundation
import PeekCore

@testable import PeekUI

/// Deterministic text metrics: every character is `0.6 × size` wide; wrapping is greedy by words.
struct SlotFixedWidthMeasurer: TextMeasuring {
    func size(of text: String, font: ChromeFont, maxWidth: CGFloat?, maxLines: Int) -> CGSize {
        let charWidth = font.size * 0.6
        guard let maxWidth else { return CGSize(width: CGFloat(text.count) * charWidth, height: font.lineHeight) }
        var lines: [Int] = [0]
        for word in text.split(separator: " ") {
            let length = word.count
            if lines[lines.count - 1] == 0 {
                lines[lines.count - 1] = length
            } else if CGFloat(lines[lines.count - 1] + 1 + length) * charWidth <= maxWidth {
                lines[lines.count - 1] += 1 + length
            } else {
                lines.append(length)
            }
        }
        let kept = Array(lines.prefix(max(1, maxLines)))
        let widest = kept.map { min(CGFloat($0) * charWidth, maxWidth) }.max() ?? 0
        let width = lines.count > kept.count ? maxWidth : widest
        return CGSize(width: width, height: CGFloat(kept.count) * font.lineHeight)
    }
}

enum SlotFixtures {
    /// A 1512 × 944 pt visible frame (a 14" MacBook Pro), origin at the bottom-left.
    static let visibleFrame = CGRect(x: 0, y: 0, width: 1512, height: 944)

    static func layout(_ slot: SlotIndex, _ mode: DisplayMode = .normal) -> SlotLayout {
        SlotGeometry.layout(slot: slot, mode: mode, visibleFrame: visibleFrame)
    }

    static func show(_ sendID: String, slot: SlotIndex = .bottom, context: PeekContext = .production, speak: String? = nil,
                   audio: Bool = true, texts: [String] = ["hello"], durationMs: Int? = nil) -> PeekShowEvent {
        PeekShowEvent(
            sendID: sendID, slot: slot, context: context,
            speak: speak.map { SpeakInfo(text: $0, status: audio ? .pending : .unsupportedLanguage) },
            show: texts.isEmpty ? nil : ShowPayload(elements: texts.map(ShowElement.text)), durationMs: durationMs)
    }

    static func ask(_ sendID: String, askID: String = "ask_1", slot: SlotIndex = .bottom, context: PeekContext = .production,
                  kind: AskKind, question: String = "Delete old.zip?", speak: String? = nil) -> PeekShowEvent {
        PeekShowEvent(
            sendID: sendID, askID: askID, slot: slot, context: context,
            speak: speak.map { SpeakInfo(text: $0, status: .pending) }, ask: AskPayload(question: question, kind: kind))
    }

    static let keepDelete: AskKind = .singleChoice(options: [AskOption(id: "keep", label: "Keep"), AskOption(id: "delete", label: "Delete")])

    static func recording(silent: Bool = false, ms: Int = 1200) -> MicRecordingResult {
        MicRecordingResult(wav: Data(repeating: 1, count: 44 + 64), durationMs: ms, peakDBFS: silent ? -70 : -18)
    }
}

extension Array where Element == BubbleEffect {
    var sent: [BubbleOutbound] {
        compactMap {
            if case .send(let outbound) = $0 { return outbound }
            return nil
        }
    }

    func schedules(_ timer: BubbleTimer) -> Double? {
        for effect in self {
            if case .schedule(timer, let seconds) = effect { return seconds }
        }
        return nil
    }

    var finishedRequeue: Bool? {
        for effect in self {
            if case .finished(let requeue) = effect { return requeue }
        }
        return nil
    }
}

extension SlotFixtures {
    /// Drives a machine through the pre-warm and the slide-in so tests start from a visible bubble.
    static func visibleMachine(_ event: PeekShowEvent, speechAvailable: Bool = true) -> BubbleMachine {
        var machine = BubbleMachine(id: .send(event.sendID), source: .send(event), speechAvailable: speechAvailable)
        _ = machine.handle(.begin, now: 0)
        _ = machine.handle(.prewarmed, now: 0)
        _ = machine.handle(.timerFired(.enter), now: 0.5)
        return machine
    }
}

extension BubbleMachine {
    /// `.begin` then `.prewarmed` at the same instant: the effects of both (tests that do not care about the pre-warm).
    mutating func beginAndSlide(now: Double) -> [BubbleEffect] {
        handle(.begin, now: now) + handle(.prewarmed, now: now)
    }
}

// MARK: - Fakes for SlotManager

@MainActor
final class SlotFakeSpeech: SpeechPlaying {
    var handled: [TTSStreamEvent] = []
    var stopped: [String] = []
    var playbacks: [String: SpeechPlayback] = [:]
    var onFinished: (@MainActor (SpeechFinished) -> Void)?
    var onFailed: (@MainActor (String, IPCErrorBody) -> Void)?

    func handle(_ event: TTSStreamEvent) { handled.append(event) }
    func stop(sendID: String) { stopped.append(sendID) }
    func playback(for sendID: String) -> SpeechPlayback? { playbacks[sendID] }
}

@MainActor
final class SlotFakeMic: MicRecording {
    var permission: MicPermission = .granted
    var isRecording = false
    var level: Double = 0.4
    var onAutoStop: (@MainActor (MicRecordingResult) -> Void)?
    var nextResult = SlotFixtures.recording()
    var starts = 0
    var cancels = 0

    func requestPermission() async -> Bool { permission == .granted }
    func start() throws(MicRecordingError) {
        starts += 1
        isRecording = true
    }
    func stop() async throws(MicRecordingError) -> MicRecordingResult {
        isRecording = false
        return nextResult
    }
    func cancel() {
        cancels += 1
        isRecording = false
    }
}

@MainActor
final class SlotFakeImages: ImageProviding {
    var released: [String] = []
    func prepare(sendID: String, paths: [String]) async -> [String: PreparedImage] {
        Dictionary(uniqueKeysWithValues: paths.enumerated().map { index, path in
            (path, PreparedImage(handle: ImageHandle(id: index + 1, width: 400, height: 200),
                                 colors: ImageColors(dominant: "#808080", palette: ["#808080"])))
        })
    }
    func image(for handle: ImageHandle) -> CGImage? { nil }
    func release(sendID: String) { released.append(sendID) }
}

@MainActor
final class SlotFakeInputHub: InputHubbing {
    var appearance: Appearance = .light
    var states: [SiliconKey: BubbleInputState] = [:]
    final class Source: DrawingInputSource {
        var onWake: (@MainActor (WakeReason) -> Void)?
        func snapshot(t: Double, dt: Double) -> InputSnapshot { InputSnapshot(slot: .init(index: .top, facing: 0)) }
    }
    func source(for key: SiliconKey) -> any DrawingInputSource { Source() }
    func update(_ key: SiliconKey, _ mutate: (inout BubbleInputState) -> Void) {
        var state = states[key] ?? BubbleInputState(slot: .top)
        mutate(&state)
        states[key] = state
    }
    func state(for key: SiliconKey) -> BubbleInputState? { states[key] }
    func remove(_ key: SiliconKey) { states.removeValue(forKey: key) }
}

@MainActor
final class SlotFakeBackdrop: BackdropSampling {
    var source: BackdropSourceSetting = .wallpaper
    var onChange: (@MainActor (SiliconKey, Backdrop) -> Void)?
    var tracked: [SiliconKey: CGRect] = [:]
    /// Every `track` call in order (nil = untracked).
    var trackCalls: [(SiliconKey, CGRect?)] = []
    var warmed: [SiliconKey: CGRect] = [:]
    /// nil: every key counts as freshly sampled; otherwise the ages per key (missing = never sampled).
    var ages: [SiliconKey: Double]?
    var value: Backdrop = .fromAppearance(.light)
    func track(_ key: SiliconKey, rectOnScreen: CGRect?) {
        tracked[key] = rectOnScreen
        trackCalls.append((key, rectOnScreen))
    }
    func backdrop(for key: SiliconKey) -> Backdrop { value }
    func warm(_ key: SiliconKey, rectOnScreen: CGRect?) { warmed[key] = rectOnScreen }
    func sampleAge(for key: SiliconKey) -> Double? { ages.map { $0[key] } ?? 0 }
}

/// Records what the Esc router holds; `press()` simulates a global Esc.
@MainActor
final class SlotFakeEscapeKey: EscapeKeyProviding {
    var onPress: (@MainActor () -> Bool)?
    var priority: (@MainActor () -> Double)?
    private(set) var isHeld = false
    /// Every change of `isHeld`, in order.
    private(set) var changes: [Bool] = []
    /// Makes the next registration fail with this problem.
    var failWith: String?

    @discardableResult
    func setHeld(_ held: Bool) -> String? {
        isHeld = held
        changes.append(held)
        return held ? failWith : nil
    }

    @discardableResult
    func press() -> Bool { isHeld ? (onPress?() ?? false) : false }
}

@MainActor
final class SlotFakeHost: DrawingHosting {
    let key: SiliconKey
    var status: DrawingHostStatus = .empty
    var input: (any DrawingInputSource)?
    var onFailure: (@MainActor (DrawingFailure) -> Void)?
    var onLog: (@MainActor (String) -> Void)?
    var events: [DrawingEvent] = []
    var attached = false

    init(key: SiliconKey) { self.key = key }
    func load(_ script: DrawingScript) async throws(DrawingFailure) { status = .ready(sha256: script.sha256) }
    func unload() { status = .empty }
    func attach(to visualView: NSView) { attached = true }
    func detach() { attached = false }
    func deliver(_ event: DrawingEvent) { events.append(event) }
    func wake() {}
    func isOverContent(unitPoint: CGPoint) -> Bool { false }
    func validate(_ script: DrawingScript, options: ValidationOptions) async -> ValidationReport { ValidationReport(ok: true) }
    /// false: `awaitFrame` never resolves by itself (tests of the pre-warm cap).
    var framesArrive = true
    var frameRequests = 0
    func awaitFrame(timeout: Duration) async -> Bool {
        frameRequests += 1
        if framesArrive { return true }
        try? await Task.sleep(for: timeout)
        return false
    }
}

/// A SlotManager on headless surfaces with fakes and fast timings.
@MainActor
final class SlotManagerHarness {
    let speech = SlotFakeSpeech()
    let mic = SlotFakeMic()
    let images = SlotFakeImages()
    let input = SlotFakeInputHub()
    let backdrop = SlotFakeBackdrop()
    var hosts: [SiliconKey: SlotFakeHost] = [:]
    var sent: [(BubbleContext, BubbleOutbound)] = []
    var failNext: String?
    /// Whether a voice message's reply names it (false: an older peekd whose reply is empty).
    var namesVoiceMessages = true
    /// The message ids the fake peekd named, in order.
    var messageIDs: [String] = []
    /// Runs inside `send` just before the reply is handed back (with the message id it will name, if any).
    var beforeReply: ((BubbleOutbound, String?) -> Void)?
    var stoppedBeforeStart: [(String, Bool)] = []
    var clock: Double = 0
    let escapeKey = SlotFakeEscapeKey()
    var keyWindowOpen = false
    private(set) var manager: SlotManager!

    init(timing: BubbleTiming = SlotManagerHarness.fastTiming) {
        let env = SlotManagerEnvironment(
            speech: speech, mic: mic, images: images, input: input, backdrop: backdrop, glassMode: .live, timing: timing,
            measurer: SlotFixedWidthMeasurer(), now: { [unowned self] in self.clock },
            makeSurface: { _, chrome in HeadlessSlotSurface(chrome: chrome) },
            visibleFrame: { _ in SlotFixtures.visibleFrame },
            host: { [unowned self] key in
                if let host = self.hosts[key] { return host }
                let host = SlotFakeHost(key: key)
                self.hosts[key] = host
                return host
            },
            send: { [unowned self] context, outbound in
                self.sent.append((context, outbound))
                defer { self.failNext = nil }
                if let problem = self.failNext {
                    self.beforeReply?(outbound, nil)
                    return .failed(problem)
                }
                var messageID: String?
                if case .voice = outbound, context.askID == nil, self.namesVoiceMessages {
                    messageID = "cmsg_\(self.sent.count)"
                    self.messageIDs.append(messageID!)
                }
                self.beforeReply?(outbound, messageID)
                return BubbleDelivery(messageID: messageID)
            },
            speechStoppedBeforeStart: { [unowned self] sendID, byUser in self.stoppedBeforeStart.append((sendID, byUser)) },
            escapeKeys: escapeKey, keyWindowIsOpen: { [unowned self] in self.keyWindowOpen })
        manager = SlotManager(environment: env)
    }

    static var fastTiming: BubbleTiming {
        var timing = BubbleTiming()
        timing.enter = 0.02
        timing.leave = 0.02
        timing.afterSpeech = 0.03
        timing.doubleClick = 0.08
        timing.noticeDuration = 5
        timing.transcribeTimeout = 5
        timing.speechStartTimeout = 5
        timing.summonIdle = 5
        timing.sentLinger = 0.02
        timing.escDouble = 0.08
        timing.escHint = 0.3
        timing.prewarmGlass = 0
        timing.prewarmTicks = 0
        timing.backdropWait = 0
        timing.prewarmMax = 0.3
        return timing
    }

    /// Everything the bubbles sent except `shown` (reported by every send bubble as its pre-warm starts).
    var sentWithoutShown: [BubbleOutbound] { sentOutbounds.filter { $0 != .shown } }

    var sentOutbounds: [BubbleOutbound] { sent.map(\.1) }

    func surface(_ slot: SlotIndex) -> HeadlessSlotSurface? { manager.surface(on: slot) as? HeadlessSlotSurface }

    /// Polls until `condition` holds (timers run on the main actor).
    func eventually(_ timeout: Duration = .seconds(3), _ condition: () -> Bool) async -> Bool {
        let deadline = ContinuousClock.now + timeout
        while ContinuousClock.now < deadline {
            if condition() { return true }
            try? await Task.sleep(for: .milliseconds(5))
        }
        return condition()
    }
}

extension SlotFixtures {
    static let dj = SlotState(index: .bottom, actorID: "si:dj", orgID: "tos", displayName: "DJ")
    static let cleanup = SlotState(index: .right, actorID: "si:cleanup", orgID: "tos", displayName: "Cleanup")
    static let djTest = SlotState(index: .bottom, context: .testing(environmentID: "0192-env"), actorID: "si:dj", orgID: "tos",
                                  displayName: "DJ", environment: TestEnvironmentInfo(id: "0192-env", name: "staging", generation: 3))
}
