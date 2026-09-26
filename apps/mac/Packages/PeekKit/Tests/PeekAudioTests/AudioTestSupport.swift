import AVFoundation
import Foundation
import PeekCore
import Testing

@testable import PeekAudio

/// Signal helpers for the audio tests.
enum Signal {
    static func sine(frequency: Double = 440, amplitude: Double, sampleRate: Double, frames: Int,
                     phase: Int = 0) -> [Float] {
        (0..<frames).map { Float(amplitude * sin(2 * .pi * frequency * Double($0 + phase) / sampleRate)) }
    }

    static func silence(frames: Int) -> [Float] { [Float](repeating: 0, count: frames) }

    /// Float samples → s16le bytes, as peekd sends them.
    static func s16le(_ samples: [Float]) -> Data {
        var data = Data(capacity: samples.count * 2)
        for sample in samples {
            let value = Int16(max(-32768, min(32767, (Double(sample) * 32768).rounded())))
            withUnsafeBytes(of: value.littleEndian) { data.append(contentsOf: $0) }
        }
        return data
    }

    /// A deinterleaved Float32 buffer (the hardware tap format).
    static func buffer(_ channels: [[Float]], sampleRate: Double) -> AVAudioPCMBuffer {
        let format = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: sampleRate,
                                   channels: AVAudioChannelCount(channels.count), interleaved: false)!
        let frames = channels[0].count
        let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(frames))!
        buffer.frameLength = AVAudioFrameCount(frames)
        for (index, samples) in channels.enumerated() {
            samples.withUnsafeBufferPointer { buffer.floatChannelData![index].update(from: $0.baseAddress!, count: frames) }
        }
        return buffer
    }

    /// Splits a long signal into tap-sized buffers.
    static func tapBuffers(_ samples: [Float], sampleRate: Double, size: Int = 1024) -> [AVAudioPCMBuffer] {
        stride(from: 0, to: samples.count, by: size).map { start in
            buffer([Array(samples[start..<min(start + size, samples.count)])], sampleRate: sampleRate)
        }
    }

    static func int16Samples(ofWAV wav: Data) -> [Int16] {
        let pcm = wav.dropFirst(WAVFile.headerSize)
        return stride(from: pcm.startIndex, to: pcm.endIndex - 1, by: 2).map { index in
            Int16(bitPattern: UInt16(pcm[index]) | UInt16(pcm[index + 1]) << 8)
        }
    }
}

/// Polls `condition` on the main actor, giving queued main-thread work a chance to run.
@MainActor
func waitUntil(timeout: Duration = .seconds(2), _ condition: @MainActor () -> Bool) async -> Bool {
    let deadline = ContinuousClock.now + timeout
    while !condition() {
        if ContinuousClock.now > deadline { return false }
        try? await Task.sleep(for: .milliseconds(2))
    }
    return true
}

/// A controllable clock for level smoothing.
@MainActor
final class ManualClock {
    var now: Double = 100
    func advance(_ seconds: Double) { now += seconds }
}

// MARK: - Fake speech output

@MainActor
final class FakeSpeechOutput: SpeechOutputEngine {
    var voices: [FakeVoice] = []
    var presentationLatency: TimeInterval = 0
    var onConfigurationChange: (@MainActor () -> Void)?
    var failure: SpeechOutputError?
    var resets = 0
    var requestedRates: [Double] = []

    func makeVoice(sampleRate: Double) throws(SpeechOutputError) -> any SpeechVoice {
        requestedRates.append(sampleRate)
        if let failure { throw failure }
        let voice = FakeVoice()
        voices.append(voice)
        return voice
    }

    func reset() { resets += 1 }

    /// The output device changed: the engine stopped, every voice is dead.
    func changeConfiguration() {
        for voice in voices { voice.renderedFrames = nil }
        onConfigurationChange?()
    }

    var voice: FakeVoice { voices.last! }
}

@MainActor
final class FakeVoice: SpeechVoice {
    struct Scheduled {
        var samples: [Float]
        var onPlayedBack: @MainActor @Sendable () -> Void
    }

    var scheduled: [Scheduled] = []
    var playedBack = 0
    var isPlaying = false
    var playCount = 0
    var stopped = false
    var invalidated = false
    var renderedFrames: Int?

    func schedule(_ samples: [Float], onPlayedBack: @escaping @MainActor @Sendable () -> Void) {
        scheduled.append(Scheduled(samples: samples, onPlayedBack: onPlayedBack))
    }

    func play() {
        isPlaying = true
        playCount += 1
        if renderedFrames == nil { renderedFrames = 0 }
    }

    func stop() {
        stopped = true
        isPlaying = false
    }

    func invalidate() { invalidated = true }

    var scheduledFrames: Int { scheduled.reduce(0) { $0 + $1.samples.count } }

    /// Renders and plays back the next `count` buffers (fires their `.dataPlayedBack` callbacks).
    func playBack(_ count: Int) {
        for _ in 0..<count where playedBack < scheduled.count {
            renderedFrames = (renderedFrames ?? 0) + scheduled[playedBack].samples.count
            scheduled[playedBack].onPlayedBack()
            playedBack += 1
        }
    }

    func playBackAll() { playBack(scheduled.count - playedBack) }
}

// MARK: - Fake microphone

@MainActor
final class FakeMicPermission: MicPermissionProviding {
    var current: MicPermission
    var grantOnRequest: Bool
    var requests = 0

    init(_ current: MicPermission, grantOnRequest: Bool = true) {
        self.current = current
        self.grantOnRequest = grantOnRequest
    }

    func request() async -> Bool {
        requests += 1
        current = grantOnRequest ? .granted : .denied
        return grantOnRequest
    }
}

@MainActor
final class FakeMicDevice: MicCaptureDevice {
    var onInterrupted: (@MainActor (MicRecordingError) -> Void)?
    var handler: (@Sendable (AVAudioPCMBuffer) -> Void)?
    var startError: MicRecordingError?
    var starts = 0
    var stops = 0

    func start(onBuffer: @escaping @Sendable (AVAudioPCMBuffer) -> Void) throws(MicRecordingError) {
        if let startError { throw startError }
        starts += 1
        handler = onBuffer
    }

    func stop() {
        stops += 1
        handler = nil
    }

    /// Delivers buffers like the tap does: from a background thread.
    func deliver(_ buffers: [AVAudioPCMBuffer]) async {
        guard let handler else { return }
        let boxes = buffers.map(BufferBox.init)
        await Task.detached { for box in boxes { handler(box.buffer) } }.value
    }
}

/// Carries a buffer to the simulated audio thread (it is only read there).
final class BufferBox: @unchecked Sendable {
    let buffer: AVAudioPCMBuffer
    init(_ buffer: AVAudioPCMBuffer) { self.buffer = buffer }
}
