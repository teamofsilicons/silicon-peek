import AVFoundation
import Foundation
import OSLog
import PeekCore

/// The microphone hardware side of ``MicRecorder``. Injected so the recorder can be tested without a device.
@MainActor
public protocol MicCaptureDevice: AnyObject {
    /// Starts delivering input buffers to `onBuffer` on an audio thread (never the main thread).
    func start(onBuffer: @escaping @Sendable (AVAudioPCMBuffer) -> Void) throws(MicRecordingError)
    /// Stops delivering. Buffers already in flight may still arrive; the recorder ignores them.
    func stop()
    /// The device stopped on its own (it disappeared and no replacement could be opened).
    var onInterrupted: (@MainActor (MicRecordingError) -> Void)? { get set }
}

/// Microphone permission (TCC). Injected for tests.
@MainActor
public protocol MicPermissionProviding: AnyObject {
    var current: MicPermission { get }
    /// Shows the system prompt when undetermined; returns whether access is granted.
    func request() async -> Bool
}

/// `AVAudioApplication.recordPermission` / `requestRecordPermission` (macOS 14, BLUEPRINT §8.3 "Mic").
@MainActor
public final class SystemMicPermission: MicPermissionProviding {
    public init() {}

    public var current: MicPermission {
        // AVAudioApplication has no "restricted"; the capture-device status does (MDM, parental controls).
        if AVCaptureDevice.authorizationStatus(for: .audio) == .restricted { return .restricted }
        switch AVAudioApplication.shared.recordPermission {
        case .granted: return .granted
        case .denied: return .denied
        case .undetermined: return .undetermined
        @unknown default: return .undetermined
        }
    }

    public func request() async -> Bool {
        await AVAudioApplication.requestRecordPermission()
    }
}

/// The live microphone: a **separate** `AVAudioEngine` (the speech engine never touches `inputNode`, which would
/// light the mic indicator), a 1024-frame tap on the input node in the hardware format, and **no** voice
/// processing (it would duck other audio and break the DJ case). After a device change the tap is reinstalled
/// in the new format.
@MainActor
public final class AVMicCaptureDevice: MicCaptureDevice {
    public var onInterrupted: (@MainActor (MicRecordingError) -> Void)?

    public static let tapBufferSize: AVAudioFrameCount = 1024

    private var engine: AVAudioEngine?
    private var handler: (@Sendable (AVAudioPCMBuffer) -> Void)?
    private var configurationObserver: (any NSObjectProtocol)?
    private let logger = PeekLogger(category: "mic")

    public init() {}

    public func start(onBuffer: @escaping @Sendable (AVAudioPCMBuffer) -> Void) throws(MicRecordingError) {
        stop()
        let engine = AVAudioEngine()
        try Self.install(on: engine, handler: onBuffer)
        self.engine = engine
        handler = onBuffer
        configurationObserver = NotificationCenter.default.addObserver(
            forName: .AVAudioEngineConfigurationChange, object: engine, queue: .main,
            using: MainActorCallback.notification { [weak self] in self?.restartAfterConfigurationChange() })
    }

    public func stop() {
        if let configurationObserver { NotificationCenter.default.removeObserver(configurationObserver) }
        configurationObserver = nil
        if let engine {
            engine.inputNode.removeTap(onBus: 0)
            engine.stop()
        }
        engine = nil
        handler = nil
    }

    private func restartAfterConfigurationChange() {
        guard let engine, let handler else { return }
        logger.notice("microphone configuration changed; reinstalling the input tap")
        engine.inputNode.removeTap(onBus: 0)
        engine.stop()
        do throws(MicRecordingError) {
            try Self.install(on: engine, handler: handler)
        } catch {
            stop()
            onInterrupted?(error)
        }
    }

    private static func install(on engine: AVAudioEngine, handler: @escaping @Sendable (AVAudioPCMBuffer) -> Void)
        throws(MicRecordingError)
    {
        let input = engine.inputNode
        let format = input.outputFormat(forBus: 0)
        guard format.channelCount > 0, format.sampleRate > 0 else {
            throw MicRecordingError(
                "no microphone is available; connect one or pick an input in System Settings › Sound › Input, "
                    + "or type your answer instead")
        }
        input.installTap(onBus: 0, bufferSize: tapBufferSize, format: format, block: makeTapBlock(handler))
        engine.prepare()
        do {
            try engine.start()
        } catch {
            input.removeTap(onBus: 0)
            throw MicRecordingError(
                "the microphone could not start (\(error.localizedDescription)); check the input device in "
                    + "System Settings › Sound, or type your answer instead")
        }
    }

    /// Built outside the main actor: the tap runs on an audio thread (see ``MainActorCallback``).
    private nonisolated static func makeTapBlock(_ handler: @escaping @Sendable (AVAudioPCMBuffer) -> Void)
        -> AVAudioNodeTapBlock
    {
        { buffer, _ in handler(buffer) }
    }
}
