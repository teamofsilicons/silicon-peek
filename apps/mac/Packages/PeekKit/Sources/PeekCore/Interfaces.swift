import AppKit
import CoreGraphics
import Foundation

// The seams between PeekKit's modules. PeekCore only declares them; each is
// implemented by exactly one module and consumed by PeekUI (see ARCHITECTURE.md):
//
//   DaemonLinking          PeekIPC      (DaemonLink)
//   DrawingHosting,
//   DrawingRuntimeProviding PeekDrawing
//   SpeechPlaying,
//   MicRecording           PeekAudio
//   InputHubbing,
//   DrawingInputSource,
//   BackdropSampling,
//   ImageProviding         PeekInput
//   PeekPresenting         PeekUI       (used by the App target and Simulation)
//
// UI-facing protocols are @MainActor: frames are built on the main thread
// (visual.md B4) and AppKit/SwiftUI live there. Implementations move heavy work
// (QuickJS, audio render, image decode) to their own threads internally.

// MARK: - IPC

/// The connection state of the UI's link to peekd.
public enum DaemonLinkState: Sendable, Equatable {
    /// `start()` has not been called.
    case idle
    /// Connecting or handshaking; `attempt` counts from 1 since the last good connection.
    case connecting(attempt: Int)
    /// `hello` succeeded.
    case connected(HelloResult)
    /// Waiting before the next attempt. `reason` says what failed, precisely.
    case waiting(retryIn: Duration, reason: String)
    /// `stop()` was called.
    case stopped

    public var isConnected: Bool {
        if case .connected = self { return true }
        return false
    }
}

/// Errors from ``DaemonLinking/send(_:blobs:timeout:)``.
public enum DaemonLinkError: Error, Sendable, Equatable, CustomStringConvertible {
    /// No connection to peekd right now (it is reconnecting or stopped).
    case notConnected(reason: String)
    /// The connection dropped before the reply arrived. The request may or may not have been processed.
    case disconnected(op: String)
    /// No reply within the timeout.
    case timedOut(op: String, after: Duration)
    /// peekd answered with an error object.
    case remote(op: String, IPCErrorBody)
    /// The reply's `result` does not match the expected shape.
    case invalidReply(op: String, reason: String)
    /// The request could not be encoded or breaks a frame limit.
    case invalidRequest(op: String, reason: String)

    public var description: String {
        switch self {
        case .notConnected(let reason): "peekd is not connected: \(reason)"
        case .disconnected(let op): "the connection to peekd closed before \"\(op)\" was answered"
        case .timedOut(let op, let after): "peekd did not answer \"\(op)\" within \(after)"
        case .remote(let op, let body): "peekd refused \"\(op)\": \(body)"
        case .invalidReply(let op, let reason): "peekd's reply to \"\(op)\" is malformed: \(reason)"
        case .invalidRequest(let op, let reason): "cannot send \"\(op)\": \(reason)"
        }
    }
}

/// The UI's long-lived, self-reconnecting connection to peekd (BLUEPRINT §1.6). Implemented by `PeekIPC.DaemonLink`.
public protocol DaemonLinking: Sendable {
    /// Starts connecting (and reconnecting with backoff) in the background. Idempotent.
    func start() async
    /// Closes the connection and stops reconnecting. Pending requests fail with `.disconnected`.
    func stop() async
    /// A new stream of every event, in arrival order, until `stop()`.
    func events() async -> AsyncStream<DaemonEvent>
    /// A new stream of state changes, starting with the current state.
    func states() async -> AsyncStream<DaemonLinkState>
    /// Sends a request and waits for its reply. `timeout` nil = ``IPCProtocol/defaultRequestTimeout``.
    func send<R: UIRequest>(_ request: R, blobs: [Data], timeout: Duration?) async throws(DaemonLinkError) -> R.Reply
    /// Installs the handler for peekd → UI requests (`drawing.validate`, `drawing.load`, `app.update.prepare`,
    /// `app.quit`). Without a handler every request is answered `unknown_op`.
    func setRequestHandler(_ handler: (@Sendable (DaemonRequest) async -> DaemonReply)?) async
}

extension DaemonLinking {
    public func send<R: UIRequest>(_ request: R) async throws(DaemonLinkError) -> R.Reply {
        try await send(request, blobs: [], timeout: nil)
    }
}

// MARK: - Drawing

/// A drawing script ready to load: bytes plus who it belongs to.
public struct DrawingScript: Sendable, Equatable {
    public var key: SiliconKey
    public var sha256: String
    public var source: Data
    /// Shown in stack traces (e.g. `cassette.js:18:34`).
    public var filename: String

    public init(key: SiliconKey, sha256: String, source: Data, filename: String) {
        self.key = key
        self.sha256 = sha256
        self.source = source
        self.filename = filename
    }
}

public struct ValidationOptions: Sendable, Equatable {
    /// Also render a PNG grid of test frames (`--preview`).
    public var preview: Bool
    /// Include frame N's display list (`--dump-frame N`).
    public var dumpFrame: Int?

    public init(preview: Bool = false, dumpFrame: Int? = nil) {
        self.preview = preview
        self.dumpFrame = dumpFrame
    }
}

/// visual.md A9 statistics.
public struct ValidationStats: Codable, Sendable, Equatable {
    public var frames: Int
    public var p50Ms: Double
    public var p95Ms: Double
    public var maxMs: Double
    public var opsMax: Int
    public var glassRebuilds: Int

    public init(frames: Int, p50Ms: Double, p95Ms: Double, maxMs: Double, opsMax: Int, glassRebuilds: Int) {
        self.frames = frames
        self.p50Ms = p50Ms
        self.p95Ms = p95Ms
        self.maxMs = maxMs
        self.opsMax = opsMax
        self.glassRebuilds = glassRebuilds
    }

    enum CodingKeys: String, CodingKey {
        case frames
        case p50Ms = "p50_ms"
        case p95Ms = "p95_ms"
        case maxMs = "max_ms"
        case opsMax = "ops_max"
        case glassRebuilds = "glass_rebuilds"
    }
}

public struct ValidationWarning: Codable, Sendable, Equatable {
    /// e.g. `glass_outline_unstable`, `text_in_compact`, `ops_truncated`.
    public var code: String
    public var message: String

    public init(code: String, message: String) {
        self.code = code
        self.message = message
    }
}

public struct ValidationFailure: Codable, Sendable, Equatable {
    public var message: String
    public var stack: String?
    public var frame: Int?
    /// e.g. `phase=showing, show=null` (visual.md A9 failure output).
    public var inputSummary: String?

    public init(message: String, stack: String? = nil, frame: Int? = nil, inputSummary: String? = nil) {
        self.message = message
        self.stack = stack
        self.frame = frame
        self.inputSummary = inputSummary
    }

    enum CodingKeys: String, CodingKey {
        case message, stack, frame
        case inputSummary = "input_summary"
    }
}

/// The result of validating a drawing: exactly the `drawing.validate` reply (§1.6) plus the preview PNG,
/// which travels as the reply's blob rather than in JSON.
public struct ValidationReport: Codable, Sendable, Equatable {
    public var ok: Bool
    public var stats: ValidationStats?
    public var warnings: [ValidationWarning]
    public var logs: [String]
    public var error: ValidationFailure?
    public var dump: JSONValue?
    public var previewPNG: Data? = nil

    public init(ok: Bool, stats: ValidationStats? = nil, warnings: [ValidationWarning] = [], logs: [String] = [],
                error: ValidationFailure? = nil, dump: JSONValue? = nil, previewPNG: Data? = nil) {
        self.ok = ok
        self.stats = stats
        self.warnings = warnings
        self.logs = logs
        self.error = error
        self.dump = dump
        self.previewPNG = previewPNG
    }

    enum CodingKeys: String, CodingKey { case ok, stats, warnings, logs, error, dump }

    public func encode(to encoder: any Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(ok, forKey: .ok)
        try c.encode(stats, forKey: .stats)
        try c.encode(warnings, forKey: .warnings)
        try c.encode(logs, forKey: .logs)
        try c.encode(error, forKey: .error)
        try c.encodeIfPresent(dump, forKey: .dump)
    }
}

/// Why a drawing switched to the fallback visual (visual.md A7, B10).
public struct DrawingFailure: Sendable, Equatable, Error {
    public var reason: DrawingFailureReason
    public var message: String
    public var stack: String?

    public init(reason: DrawingFailureReason, message: String, stack: String? = nil) {
        self.reason = reason
        self.message = message
        self.stack = stack
    }
}

public enum DrawingHostStatus: Sendable, Equatable {
    case empty
    case loading
    case ready(sha256: String)
    /// Showing the glass circle with the Silicon's initial.
    case fallback(DrawingFailure)
}

/// One-off drawing events (visual.md A3, without `word`: BLUEPRINT §0.1).
public enum DrawingEvent: Sendable, Equatable {
    case enter
    case leave
    /// A send arrived: the new `input.show` / `input.ask` / `input.speech`.
    case send(show: InputSnapshot.Show?, ask: InputSnapshot.Ask?, speech: InputSnapshot.Speech?)
    case answer(value: AskValue, via: AnswerVia)
    /// A click on the visual, in units; `count` 2 = double click.
    case click(x: Double, y: Double, count: Int)
    case move(from: SlotIndex, to: SlotIndex)

    /// The name passed to `__peek_event`.
    public var name: String {
        switch self {
        case .enter: "enter"
        case .leave: "leave"
        case .send: "send"
        case .answer: "answer"
        case .click: "click"
        case .move: "move"
        }
    }

    /// The JSON payload passed to `__peek_event` (`nil` → `undefined`).
    public var payloadJSON: [UInt8]? {
        var out: [UInt8] = []
        switch self {
        case .enter, .leave:
            return nil
        case .send(let show, let ask, let speech):
            var w = FieldWriter(&out)
            w.key("show", &out)
            Self.writeCodable(show, into: &out)
            w.key("ask", &out)
            Self.writeCodable(ask, into: &out)
            w.key("speech", &out)
            Self.writeCodable(speech, into: &out)
            w.close(&out)
        case .answer(let value, let via):
            var w = FieldWriter(&out)
            w.key("value", &out)
            value.jsonValue.write(into: &out)
            w.string("via", via.rawValue, &out)
            w.close(&out)
        case .click(let x, let y, let count):
            var w = FieldWriter(&out)
            w.number("x", x, &out)
            w.number("y", y, &out)
            w.number("count", Double(count), &out)
            w.close(&out)
        case .move(let from, let to):
            var w = FieldWriter(&out)
            w.number("from", Double(from.rawValue), &out)
            w.number("to", Double(to.rawValue), &out)
            w.close(&out)
        }
        return out
    }

    private static func writeCodable<T: Encodable>(_ value: T?, into out: inout [UInt8]) {
        guard let value, let fields = try? JSONEncoder().encode(value), let parsed = try? StrictJSON.parse(fields) else {
            JSONWriter.writeNull(into: &out)
            return
        }
        parsed.write(into: &out)
    }
}

/// Things that wake a sleeping drawing for one or more frames (visual.md A3, minus "a word is spoken").
public enum WakeReason: Sendable, Hashable {
    case phaseChanged
    case send
    case answer
    case speechLevel
    case micLevel
    case hover
    case pointerMoved
    case click
    case slotChanged
    case modeChanged
    case appearanceChanged
    case backdropChanged
}

/// Supplies a drawing's per-frame `input` (implemented by PeekInput's InputHub).
@MainActor
public protocol DrawingInputSource: AnyObject {
    /// The snapshot for the frame about to run. `t` and `dt` come from the frame scheduler.
    func snapshot(t: Double, dt: Double) -> InputSnapshot
    /// Called when something changed that should wake a sleeping drawing.
    var onWake: (@MainActor (WakeReason) -> Void)? { get set }
}

/// One Silicon's drawing: QuickJS VM, frame scheduler and compositor (implemented by PeekDrawing).
@MainActor
public protocol DrawingHosting: AnyObject {
    var key: SiliconKey { get }
    var status: DrawingHostStatus { get }
    var input: (any DrawingInputSource)? { get set }
    /// Called once when the drawing switches to the fallback visual; the UI forwards it as `drawing.error`.
    var onFailure: (@MainActor (DrawingFailure) -> Void)? { get set }
    /// `peek.log` output (shown in Simulation; ignored in normal operation).
    var onLog: (@MainActor (String) -> Void)? { get set }

    /// Loads a script: top-level code runs once, then the drawing is paused (visual.md A1).
    /// Replaces any previous script (all state is lost).
    func load(_ script: DrawingScript) async throws(DrawingFailure)
    /// Destroys the VM and clears the visual.
    func unload()
    /// Puts the drawing's layers into `visualView` (the 100 × 100 square) and starts the frame clock when awake.
    func attach(to visualView: NSView)
    func detach()
    /// Delivers a one-off event and wakes the drawing.
    func deliver(_ event: DrawingEvent)
    /// Wakes a sleeping drawing (the input source's `onWake` normally does this).
    func wake()
    /// B9 hit test in drawing units: inside a glass/blur path or over a drawn pixel.
    func isOverContent(unitPoint: CGPoint) -> Bool
    /// Runs visual.md A9 in a separate, temporary runtime (never touches this host's VM).
    func validate(_ script: DrawingScript, options: ValidationOptions) async -> ValidationReport
}

/// Creates drawing hosts and validates scripts for peekd (implemented by PeekDrawing).
@MainActor
public protocol DrawingRuntimeProviding: AnyObject {
    /// `live` when the private active-appearance override works, else `frosted` (D13).
    var glassMode: GlassMode { get }
    func makeHost(for key: SiliconKey, initial: String, images: any ImageProviding) -> any DrawingHosting
    /// `drawing.validate` from peekd: validate the file at `scriptPath`.
    func validate(scriptAt url: URL, options: ValidationOptions) async -> ValidationReport
}

// MARK: - Audio

/// Speech playback state for one send, sampled at "now" (for `input.speech`).
public struct SpeechPlayback: Sendable, Equatable {
    public var sendID: String
    /// Smoothed RMS (0…1) of the audio audible right now.
    public var level: Double
    /// Played frames / total frames: exact after `tts.end`, estimated before, monotonic, ≤ 0.99 until done.
    public var progress: Double
    public var done: Bool
    /// Playback has begun (≈100 ms buffered).
    public var started: Bool
    public var playedMs: Int
    public var totalMs: Int?

    public init(sendID: String, level: Double, progress: Double, done: Bool, started: Bool, playedMs: Int, totalMs: Int?) {
        self.sendID = sendID
        self.level = level
        self.progress = progress
        self.done = done
        self.started = started
        self.playedMs = playedMs
        self.totalMs = totalMs
    }
}

/// Reported once per send when playback ends, naturally or by a double click; becomes `speech.done`.
public struct SpeechFinished: Sendable, Equatable {
    public var sendID: String
    public var stoppedByUser: Bool
    public var playedMs: Int
    public var totalMs: Int

    public init(sendID: String, stoppedByUser: Bool, playedMs: Int, totalMs: Int) {
        self.sendID = sendID
        self.stoppedByUser = stoppedByUser
        self.playedMs = playedMs
        self.totalMs = totalMs
    }

    public var request: SpeechDoneRequest {
        SpeechDoneRequest(sendID: sendID, stoppedByUser: stoppedByUser, playedMs: playedMs, totalMs: totalMs)
    }
}

/// The TTS stream events PeekUI forwards from the link to the player.
public enum TTSStreamEvent: Sendable, Equatable {
    case begin(TTSBegin)
    case chunk(TTSChunk)
    case end(TTSEnd)
    case failure(TTSErrorEvent)

    public var sendID: String {
        switch self {
        case .begin(let e): e.sendID
        case .chunk(let e): e.sendID
        case .end(let e): e.sendID
        case .failure(let e): e.sendID
        }
    }
}

/// Plays peekd's streamed Deepgram PCM (s16le mono 24 kHz) and exposes its level/progress timeline
/// (BLUEPRINT §8.7). Implemented by PeekAudio.
@MainActor
public protocol SpeechPlaying: AnyObject {
    func handle(_ event: TTSStreamEvent)
    /// Stops playback now (double click on the down-arrow).
    func stop(sendID: String)
    /// Current playback state, or `nil` when nothing is known about `sendID`.
    func playback(for sendID: String) -> SpeechPlayback?
    var onFinished: (@MainActor (SpeechFinished) -> Void)? { get set }
    /// Synthesis failed before any audio played: show the text as a pill instead (§1.9.3).
    var onFailed: (@MainActor (String, IPCErrorBody) -> Void)? { get set }
}

public enum MicPermission: String, Sendable, Equatable {
    case undetermined
    case granted
    case denied
    /// Blocked by a device-management profile or parental controls; the Carbon cannot allow it.
    case restricted

    /// The Carbon can never be recorded right now (denied or restricted).
    public var isBlocked: Bool { self == .denied || self == .restricted }
}

/// A finished recording, ready for `voice.submit`.
public struct MicRecordingResult: Sendable, Equatable {
    /// 16 kHz mono Int16 RIFF/WAVE, ≤ ``FrameLimits/maxWAVBytes``.
    public var wav: Data
    public var durationMs: Int
    /// Peak RMS over the recording, in dBFS.
    public var peakDBFS: Double
    /// Peak below −50 dBFS: "nothing heard", do not upload (§1.9.5).
    public var isSilent: Bool { peakDBFS < MicRecordingResult.silenceThresholdDBFS }

    public static let silenceThresholdDBFS = -50.0
    public static let maxDuration: Duration = .seconds(120)

    public init(wav: Data, durationMs: Int, peakDBFS: Double) {
        self.wav = wav
        self.durationMs = durationMs
        self.peakDBFS = peakDBFS
    }
}

public struct MicRecordingError: Error, Sendable, Equatable, CustomStringConvertible {
    public var description: String
    public init(_ description: String) { self.description = description }
}

/// Microphone capture on its own AVAudioEngine, no voice processing (§8.7). Implemented by PeekAudio.
@MainActor
public protocol MicRecording: AnyObject {
    var permission: MicPermission { get }
    func requestPermission() async -> Bool
    var isRecording: Bool { get }
    /// Smoothed 0…1 level (`input.mic.level` and the live waveform); 0 when not recording.
    var level: Double { get }
    func start() throws(MicRecordingError)
    /// Stops and returns the WAV.
    func stop() async throws(MicRecordingError) -> MicRecordingResult
    /// Stops and discards.
    func cancel()
    /// Called when the 120 s cap stopped the recording on its own.
    var onAutoStop: (@MainActor (MicRecordingResult) -> Void)? { get set }
}

// MARK: - Input

/// What PeekUI knows about one bubble; InputHub combines it with audio, pointer,
/// appearance, backdrop and images into ``InputSnapshot``.
public struct BubbleInputState: Sendable, Equatable {
    public var slot: SlotIndex
    public var mode: DisplayMode
    public var phase: Phase
    public var context: InputContext
    public var glass: GlassMode
    /// The visual's 100 × 100 square on screen (``SlotLayout/visualFrameOnScreen``).
    public var visualFrameOnScreen: CGRect
    public var facing: Double
    public var sendID: String?
    public var speakText: String?
    /// Paths are peekd cache paths; InputHub resolves them through ``ImageProviding``.
    public var show: ShowPayload?
    public var ask: AskPayload?
    public var askValue: AskValue?
    public var askHighlight: String?
    public var typingText: String?
    /// The mic is recording for this bubble (feeds `input.mic.level`).
    public var listening: Bool

    public init(slot: SlotIndex, mode: DisplayMode = .normal, phase: Phase = .hidden, context: InputContext = .production,
                glass: GlassMode = .live, visualFrameOnScreen: CGRect = .zero, facing: Double = 0, sendID: String? = nil,
                speakText: String? = nil, show: ShowPayload? = nil, ask: AskPayload? = nil, askValue: AskValue? = nil,
                askHighlight: String? = nil, typingText: String? = nil, listening: Bool = false) {
        self.slot = slot
        self.mode = mode
        self.phase = phase
        self.context = context
        self.glass = glass
        self.visualFrameOnScreen = visualFrameOnScreen
        self.facing = facing
        self.sendID = sendID
        self.speakText = speakText
        self.show = show
        self.ask = ask
        self.askValue = askValue
        self.askHighlight = askHighlight
        self.typingText = typingText
        self.listening = listening
    }
}

/// The shared input source (visual.md B8). Implemented by PeekInput.
@MainActor
public protocol InputHubbing: AnyObject {
    var appearance: Appearance { get }
    /// The input source a drawing host reads from; stable per key.
    func source(for key: SiliconKey) -> any DrawingInputSource
    /// Updates what PeekUI knows about a bubble; wakes the drawing when something visible changed.
    func update(_ key: SiliconKey, _ mutate: (inout BubbleInputState) -> Void)
    /// The current state, if the key is known.
    func state(for key: SiliconKey) -> BubbleInputState?
    func remove(_ key: SiliconKey)
}

/// Samples what each visible bubble sits on (§8.8). Implemented by PeekInput.
@MainActor
public protocol BackdropSampling: AnyObject {
    var source: BackdropSourceSetting { get set }
    /// Starts (or with `nil`, stops) sampling under `rectOnScreen` for `key`.
    func track(_ key: SiliconKey, rectOnScreen: CGRect?)
    /// The current estimate for `key` (falls back to the appearance).
    func backdrop(for key: SiliconKey) -> Backdrop
    /// Tone or colour changed: re-shade pills and wake the drawing.
    var onChange: (@MainActor (SiliconKey, Backdrop) -> Void)? { get set }
}

/// Copies, decodes (≤ 512 px) and analyses show and option images (visual.md B7). Implemented by PeekInput.
@MainActor
public protocol ImageProviding: AnyObject {
    /// Prepares every image of a send. Unreadable images are left out (the element is drawn without them).
    func prepare(sendID: String, paths: [String]) async -> [String: PreparedImage]
    /// The decoded image for a live handle; `nil` once its send was released.
    func image(for handle: ImageHandle) -> CGImage?
    /// Invalidates the handles of a send.
    func release(sendID: String)
}

// MARK: - Presentation

/// Shows bubbles. Implemented by PeekUI's slot coordinator; used by the App target (events from peekd)
/// and by Simulation (local samples with context `.simulation`).
@MainActor
public protocol PeekPresenting: AnyObject {
    /// The full slot table from `slots.state`; (re)registers hotkeys and moves drawings.
    func updateSlots(_ slots: [SlotState])
    /// Slides a bubble in (or queues it behind the one on screen).
    func present(_ show: PeekShowEvent)
    /// Slides a bubble out without sending anything.
    func cancel(sendID: String, reason: PeekCancelReason)
    /// A transcription outcome for a voice answer (matched → slide out; unmatched/failed → notice).
    func applySTTResult(_ result: STTResultEvent)
    /// True when no bubble is visible, no recording runs and no ask is on screen (`app.update.prepare`).
    var isIdleForUpdate: Bool { get }
    /// Hides everything immediately (pause, quit).
    func dismissAll()
}
