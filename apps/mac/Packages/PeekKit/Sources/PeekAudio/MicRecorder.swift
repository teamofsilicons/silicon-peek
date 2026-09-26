import AVFoundation
import Foundation
import OSLog
import PeekCore

/// Records the Carbon's voice answer (BLUEPRINT §1.9.5, §8.7): permission through `AVAudioApplication`, a
/// separate input engine with a 1024-frame tap, the live level for `input.mic.level` and peek's waveform, and a
/// 16 kHz mono Int16 WAV for `voice.submit`. Recordings stop on their own at 120 s (``onAutoStop``).
/// A result whose loudest buffer is below −50 dBFS is ``MicRecordingResult/isSilent``: the UI must not upload it.
@MainActor
public final class MicRecorder: MicRecording {
    public var onAutoStop: (@MainActor (MicRecordingResult) -> Void)?
    /// Every tap buffer's level (≈ 47 per second), for the live waveform.
    public var onLevel: (@MainActor (Double) -> Void)?
    /// The device vanished mid-recording. The audio so far is delivered through ``onAutoStop`` right after.
    public var onInterrupted: (@MainActor (MicRecordingError) -> Void)?

    /// The latest buffer levels, oldest first (at most ``historyLength``), for drawing the waveform.
    public private(set) var recentLevels: [Double] = []
    public static let historyLength = 128

    public private(set) var isRecording = false

    public let configuration: MicCaptureSession.Configuration
    private let permissions: any MicPermissionProviding
    private let makeDevice: @MainActor () -> any MicCaptureDevice
    private var device: (any MicCaptureDevice)?
    private var session: MicCaptureSession?
    /// Bumped on every start and stop, so buffers from an earlier recording are ignored.
    private var recordingID = 0
    private let logger = PeekLogger(category: "mic")

    public init(permissions: any MicPermissionProviding = SystemMicPermission(),
                configuration: MicCaptureSession.Configuration = MicCaptureSession.Configuration(),
                makeDevice: @escaping @MainActor () -> any MicCaptureDevice = { AVMicCaptureDevice() }) {
        self.permissions = permissions
        self.configuration = configuration
        self.makeDevice = makeDevice
    }

    public var permission: MicPermission { permissions.current }

    public func requestPermission() async -> Bool {
        switch permissions.current {
        case .granted: return true
        case .denied, .restricted: return false
        case .undetermined: return await permissions.request()
        }
    }

    public var level: Double { isRecording ? session?.level ?? 0 : 0 }

    public func start() throws(MicRecordingError) {
        guard !isRecording else {
            throw MicRecordingError("a recording is already running; stop it before starting another")
        }
        switch permissions.current {
        case .granted:
            break
        case .denied:
            throw MicRecordingError(
                "Peek.app is not allowed to use the microphone; turn it on in System Settings › Privacy & Security › "
                    + "Microphone, or type your answer instead")
        case .restricted:
            throw MicRecordingError(
                "the microphone is restricted on this Mac (device management or parental controls); type your answer instead")
        case .undetermined:
            throw MicRecordingError(
                "Peek.app has not been given microphone access yet; allow it when macOS asks, then try again")
        }
        let session = MicCaptureSession(configuration: configuration)
        let device = makeDevice()
        recordingID += 1
        let id = recordingID
        device.onInterrupted = { [weak self] error in self?.deviceInterrupted(error, recordingID: id) }
        try device.start(onBuffer: Self.makeBufferHandler(session: session, recorder: self, recordingID: id))
        self.session = session
        self.device = device
        recentLevels = []
        isRecording = true
    }

    public func stop() async throws(MicRecordingError) -> MicRecordingResult {
        guard isRecording else {
            throw MicRecordingError("no recording is running; start one with the mic button or \\ first")
        }
        return finishRecording()
    }

    public func cancel() {
        guard isRecording else { return }
        tearDown()
        session = nil
    }

    // MARK: Internals

    private func finishRecording() -> MicRecordingResult {
        tearDown()
        let session = self.session ?? MicCaptureSession(configuration: configuration)
        self.session = nil
        let result = session.finish()
        if let problem = session.conversionError { logger.error("\(problem)") }
        return result
    }

    private func tearDown() {
        device?.stop()
        device = nil
        recordingID += 1
        isRecording = false
        recentLevels = []
    }

    private func didIngest(_ result: MicCaptureSession.IngestResult, recordingID id: Int) {
        guard id == recordingID, isRecording else { return }
        recentLevels.append(result.bufferLevel)
        if recentLevels.count > Self.historyLength { recentLevels.removeFirst(recentLevels.count - Self.historyLength) }
        onLevel?(result.level)
        if result.reachedCap {
            logger.notice("recording reached the \(self.configuration.maxDuration) cap; stopping")
            onAutoStop?(finishRecording())
        }
    }

    private func deviceInterrupted(_ error: MicRecordingError, recordingID id: Int) {
        guard id == recordingID, isRecording else { return }
        logger.error("microphone interrupted: \(error.description)")
        onInterrupted?(error)
        onAutoStop?(finishRecording())
    }

    /// Built outside the main actor: it runs on the tap's audio thread and hops to main only with the result.
    private nonisolated static func makeBufferHandler(session: MicCaptureSession, recorder: MicRecorder,
                                                      recordingID: Int) -> @Sendable (AVAudioPCMBuffer) -> Void
    {
        { [weak recorder] buffer in
            let result = session.ingest(buffer)
            DispatchQueue.main.async {
                MainActor.assumeIsolated { recorder?.didIngest(result, recordingID: recordingID) }
            }
        }
    }
}
