import AVFoundation
import Foundation
import OSLog
import PeekCore

/// Why speech output could not be set up. `description` is a full sentence for logs and the fallback pill.
public struct SpeechOutputError: Error, Sendable, Equatable, CustomStringConvertible {
    public var description: String
    public init(_ description: String) { self.description = description }
}

/// The audio hardware side of ``SpeechPlayer``: an output engine that hands out one ``SpeechVoice`` (player node)
/// per stream. Injected so the timeline, level and progress logic can be tested without an audio device.
@MainActor
public protocol SpeechOutputEngine: AnyObject {
    /// A new voice for mono Float32 audio at `sampleRate`, connected to the output and ready to schedule.
    func makeVoice(sampleRate: Double) throws(SpeechOutputError) -> any SpeechVoice
    /// Seconds between a frame being rendered and it being heard (device latency). 0 when unknown.
    var presentationLatency: TimeInterval { get }
    /// Runs on the main actor after the output device changed and the engine stopped itself
    /// (`AVAudioEngineConfigurationChange`). Every existing voice is dead: call ``reset()`` and make new ones.
    var onConfigurationChange: (@MainActor () -> Void)? { get set }
    /// Drops the current engine so the next ``makeVoice(sampleRate:)`` builds a fresh one.
    func reset()
}

/// One stream's player node.
@MainActor
public protocol SpeechVoice: AnyObject {
    /// Queues mono samples after the ones already queued. `onPlayedBack` runs on the main actor once they have
    /// been heard (`.dataPlayedBack`). It may also run after ``stop()``; callers ignore such stale callbacks.
    func schedule(_ samples: [Float], onPlayedBack: @escaping @MainActor @Sendable () -> Void)
    /// Starts playing the queue.
    func play()
    /// Frames rendered since ``play()`` (the player's `sampleTime`), or nil while it is not rendering.
    var renderedFrames: Int? { get }
    /// Stops at once and drops the queue.
    func stop()
    /// Detaches the node from the engine. The voice cannot be used afterwards.
    func invalidate()
}

/// The live output: one `AVAudioEngine` for all speech (separate from the mic engine, so playing never lights the
/// mic indicator), one `AVAudioPlayerNode` per stream feeding the main mixer, which resamples 24 kHz to the device
/// rate (BLUEPRINT §8.3 "TTS playback"). The engine runs only while a voice exists.
@MainActor
public final class AVSpeechOutputEngine: SpeechOutputEngine {
    public var onConfigurationChange: (@MainActor () -> Void)?

    private var engine: AVAudioEngine?
    private var configurationObserver: (any NSObjectProtocol)?
    private var liveVoices = 0
    private let makeEngine: @MainActor () -> AVAudioEngine
    private let completionCallbackType: AVAudioPlayerNodeCompletionCallbackType
    private let logger = PeekLogger(category: "speech")

    /// - Parameters:
    ///   - makeEngine: builds each engine (default: a plain `AVAudioEngine` on the default output device; tests pass
    ///     one in offline manual-rendering mode so no audio hardware is touched).
    ///   - completionCallbackType: when a buffer counts as done. `.dataPlayedBack` (heard, BLUEPRINT §8.7) in the app;
    ///     offline rendering never reports it, so tests use `.dataRendered`.
    public init(makeEngine: @escaping @MainActor () -> AVAudioEngine = { AVAudioEngine() },
                completionCallbackType: AVAudioPlayerNodeCompletionCallbackType = .dataPlayedBack) {
        self.makeEngine = makeEngine
        self.completionCallbackType = completionCallbackType
    }

    public var presentationLatency: TimeInterval {
        guard let engine, engine.isRunning else { return 0 }
        let latency = engine.outputNode.presentationLatency
        return latency.isFinite && latency > 0 ? latency : 0
    }

    public func makeVoice(sampleRate: Double) throws(SpeechOutputError) -> any SpeechVoice {
        guard sampleRate > 0,
            let format = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: sampleRate, channels: 1,
                                       interleaved: false)
        else {
            throw SpeechOutputError("cannot play speech at \(sampleRate) Hz: no mono Float32 format exists for that rate")
        }
        let engine = currentEngine()
        let node = AVAudioPlayerNode()
        engine.attach(node)
        engine.connect(node, to: engine.mainMixerNode, format: format)
        if !engine.isRunning {
            engine.prepare()
            do {
                try engine.start()
            } catch {
                engine.detach(node)
                throw SpeechOutputError(
                    "Peek.app could not start audio output (\(error.localizedDescription)); "
                        + "check the output device in System Settings › Sound")
            }
        }
        liveVoices += 1
        return AVSpeechVoice(node: node, format: format, completionCallbackType: completionCallbackType, owner: self)
    }

    public func reset() {
        if let configurationObserver { NotificationCenter.default.removeObserver(configurationObserver) }
        configurationObserver = nil
        engine?.stop()
        engine = nil
        liveVoices = 0
    }

    func release(_ node: AVAudioPlayerNode) {
        guard let engine, node.engine === engine else { return }
        engine.detach(node)
        liveVoices = max(0, liveVoices - 1)
        if liveVoices == 0 {
            // Nothing is speaking: let the output device sleep.
            engine.stop()
        }
    }

    private func currentEngine() -> AVAudioEngine {
        if let engine { return engine }
        let engine = makeEngine()
        configurationObserver = NotificationCenter.default.addObserver(
            forName: .AVAudioEngineConfigurationChange, object: engine, queue: .main,
            using: MainActorCallback.notification { [weak self] in
                guard let self else { return }
                self.logger.notice("audio output configuration changed; rebuilding the speech engine")
                self.onConfigurationChange?()
            })
        self.engine = engine
        return engine
    }
}

@MainActor
final class AVSpeechVoice: SpeechVoice {
    private let node: AVAudioPlayerNode
    private let format: AVAudioFormat
    private let completionCallbackType: AVAudioPlayerNodeCompletionCallbackType
    private weak var owner: AVSpeechOutputEngine?
    private var invalidated = false

    init(node: AVAudioPlayerNode, format: AVAudioFormat, completionCallbackType: AVAudioPlayerNodeCompletionCallbackType,
         owner: AVSpeechOutputEngine) {
        self.node = node
        self.format = format
        self.completionCallbackType = completionCallbackType
        self.owner = owner
    }

    func schedule(_ samples: [Float], onPlayedBack: @escaping @MainActor @Sendable () -> Void) {
        guard !invalidated, !samples.isEmpty,
            let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(samples.count)),
            let channel = buffer.floatChannelData?[0]
        else { return }
        buffer.frameLength = AVAudioFrameCount(samples.count)
        samples.withUnsafeBufferPointer { source in
            if let base = source.baseAddress { channel.update(from: base, count: samples.count) }
        }
        node.scheduleBuffer(buffer, completionCallbackType: completionCallbackType,
                            completionHandler: MainActorCallback.playerCompletion(onPlayedBack))
    }

    func play() {
        guard !invalidated, node.engine?.isRunning == true else { return }
        node.play()
    }

    var renderedFrames: Int? {
        guard !invalidated, node.engine?.isRunning == true, let nodeTime = node.lastRenderTime,
            nodeTime.isSampleTimeValid, let playerTime = node.playerTime(forNodeTime: nodeTime)
        else { return nil }
        return max(0, Int(playerTime.sampleTime))
    }

    func stop() {
        guard !invalidated else { return }
        node.stop()
    }

    func invalidate() {
        guard !invalidated else { return }
        invalidated = true
        node.stop()
        owner?.release(node)
    }
}

/// Builds callbacks for AVFoundation and Foundation outside any actor. A closure written inside a `@MainActor`
/// method is itself main-actor isolated in Swift 6, and calling it from an audio thread traps; these wrappers
/// are nonisolated and hop to the main thread before touching main-actor state.
enum MainActorCallback {
    /// Always asynchronous: `AVAudioPlayerNode.stop()` can run pending completions on the calling thread, and the
    /// player must never re-enter itself in the middle of a state change.
    static func playerCompletion(_ body: @escaping @MainActor @Sendable () -> Void)
        -> @Sendable (AVAudioPlayerNodeCompletionCallbackType) -> Void
    {
        { _ in DispatchQueue.main.async { MainActor.assumeIsolated { body() } } }
    }

    static func notification(_ body: @escaping @MainActor @Sendable () -> Void) -> @Sendable (Notification) -> Void {
        { _ in onMain(body) }
    }

    static func onMain(_ body: @escaping @MainActor @Sendable () -> Void) {
        if Thread.isMainThread {
            MainActor.assumeIsolated { body() }
        } else {
            DispatchQueue.main.async { MainActor.assumeIsolated { body() } }
        }
    }
}
