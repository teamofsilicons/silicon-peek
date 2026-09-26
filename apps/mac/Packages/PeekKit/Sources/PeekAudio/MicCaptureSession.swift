import AVFoundation
import Foundation
import PeekCore
import Synchronization

/// One recording's audio processing, fed from the input tap's thread (BLUEPRINT §8.7, notes/speech §4.3):
///
/// * RMS of each tap buffer → dBFS → the 0…1 level (smoothed: 30 ms attack, 150 ms release) that drives
///   `input.mic.level` and the live waveform; the loudest buffer is kept as the peak for the silence guard.
/// * `AVAudioConverter` resamples and downmixes to Int16 16 kHz mono; the bytes are appended to a buffer.
/// * The recording stops growing at 120 s (≈ 3.8 MB), and ``finish()`` returns the WAV.
///
/// Thread-safe: ``ingest(_:)`` runs on the audio thread, everything else on any thread.
public final class MicCaptureSession: Sendable {
    public struct Configuration: Sendable, Equatable {
        public var outputSampleRate: Double
        /// Longest recording kept; the rest is dropped and ``IngestResult/reachedCap`` fires once.
        public var maxDuration: Duration
        public var attack: Double
        public var release: Double

        public init(outputSampleRate: Double = 16_000, maxDuration: Duration = MicRecordingResult.maxDuration,
                    attack: Double = 0.030, release: Double = 0.150) {
            self.outputSampleRate = outputSampleRate
            self.maxDuration = maxDuration
            self.attack = attack
            self.release = release
        }

        public var maxFrames: Int {
            let seconds = Double(maxDuration.components.seconds) + Double(maxDuration.components.attoseconds) / 1e18
            return Int(seconds * outputSampleRate)
        }
    }

    /// What one tap buffer changed.
    public struct IngestResult: Sendable, Equatable {
        /// The smoothed level after this buffer.
        public var level: Double
        /// The raw (unsmoothed) level of this buffer, for the waveform history.
        public var bufferLevel: Double
        /// True exactly once: this buffer filled the recording to the cap.
        public var reachedCap: Bool
    }

    private struct State {
        var converter: AVAudioConverter?
        var converterInput: AVAudioFormat?
        var pcm = Data()
        var frames = 0
        var peakDBFS = AudioLevel.silenceDBFS
        var smoother: LevelSmoother
        var capped = false
        var finished = false
        var conversionError: String?
    }

    public let configuration: Configuration
    private let outputFormat: AVAudioFormat
    private let state: Mutex<State>

    public init(configuration: Configuration = Configuration()) {
        self.configuration = configuration
        guard let format = AVAudioFormat(commonFormat: .pcmFormatInt16, sampleRate: configuration.outputSampleRate,
                                         channels: 1, interleaved: true)
        else { preconditionFailure("Int16 mono at \(configuration.outputSampleRate) Hz is always a valid format") }
        outputFormat = format
        state = Mutex(State(smoother: LevelSmoother(attack: configuration.attack, release: configuration.release)))
        state.withLock { $0.pcm.reserveCapacity(min(configuration.maxFrames, 16_000 * 30) * 2) }
    }

    /// The smoothed level (0…1) after the latest buffer.
    public var level: Double { state.withLock { $0.smoother.value } }
    /// Output frames kept so far.
    public var frameCount: Int { state.withLock { $0.frames } }
    public var peakDBFS: Double { state.withLock { $0.peakDBFS } }

    /// Measures and converts one input buffer (any rate, any channel count, Float32 or Int16).
    @discardableResult
    public func ingest(_ buffer: AVAudioPCMBuffer) -> IngestResult {
        let bufferRMS = Self.loudestChannelRMS(buffer)
        let dbfs = AudioLevel.dbfs(rms: bufferRMS)
        let bufferLevel = AudioLevel.level(dbfs: dbfs)
        let duration = buffer.format.sampleRate > 0 ? Double(buffer.frameLength) / buffer.format.sampleRate : 0
        return state.withLock { state in
            guard !state.finished else {
                return IngestResult(level: state.smoother.value, bufferLevel: 0, reachedCap: false)
            }
            let level = state.smoother.step(toward: bufferLevel, dt: duration)
            guard !state.capped else { return IngestResult(level: level, bufferLevel: bufferLevel, reachedCap: false) }
            state.peakDBFS = max(state.peakDBFS, dbfs)
            convert(buffer, state: &state)
            let reachedCap = state.frames >= configuration.maxFrames
            if reachedCap { state.capped = true }
            return IngestResult(level: level, bufferLevel: bufferLevel, reachedCap: reachedCap)
        }
    }

    /// Drains the converter and returns the recording. Later buffers are ignored.
    public func finish() -> MicRecordingResult {
        state.withLock { state in
            if !state.finished {
                if !state.capped { drain(&state) }
                state.finished = true
                state.smoother.reset()
            }
            let durationMs = Int((Double(state.frames) * 1000 / configuration.outputSampleRate).rounded())
            return MicRecordingResult(wav: WAVFile.make(pcm: state.pcm, sampleRate: Int(configuration.outputSampleRate)),
                                      durationMs: durationMs, peakDBFS: state.peakDBFS)
        }
    }

    /// Why conversion failed, if it ever did (the recording then holds what converted before).
    public var conversionError: String? { state.withLock { $0.conversionError } }

    // MARK: Conversion

    private func convert(_ buffer: AVAudioPCMBuffer, state: inout State) {
        if state.converterInput != buffer.format {
            // The input device changed format (e.g. AirPods connected): flush the old converter, start a new one.
            drain(&state)
            guard let converter = AVAudioConverter(from: buffer.format, to: outputFormat) else {
                state.conversionError =
                    "cannot convert the microphone's \(buffer.format) to 16 kHz mono; the recording stops here"
                state.converter = nil
                state.converterInput = buffer.format
                return
            }
            converter.downmix = true
            state.converter = converter
            state.converterInput = buffer.format
        }
        guard let converter = state.converter, buffer.frameLength > 0 else { return }
        let ratio = outputFormat.sampleRate / buffer.format.sampleRate
        let capacity = AVAudioFrameCount((Double(buffer.frameLength) * ratio).rounded(.up)) + 64
        run(converter, capacity: capacity, feed: InputFeed(buffer: buffer), state: &state)
    }

    /// Flushes the samples a converter holds back for its filter.
    private func drain(_ state: inout State) {
        guard let converter = state.converter else { return }
        run(converter, capacity: 4096, feed: InputFeed(buffer: nil), state: &state)
        state.converter = nil
        state.converterInput = nil
    }

    private func run(_ converter: AVAudioConverter, capacity: AVAudioFrameCount, feed: InputFeed,
                     state: inout State) {
        // A drain can take several passes; a normal buffer needs one (the input block runs dry after it).
        for _ in 0..<8 {
            guard let output = AVAudioPCMBuffer(pcmFormat: outputFormat, frameCapacity: capacity) else { return }
            var error: NSError?
            let status = converter.convert(to: output, error: &error, withInputFrom: feed.block)
            if status == .error {
                state.conversionError = "the microphone audio could not be converted to 16 kHz: "
                    + (error?.localizedDescription ?? "unknown AVAudioConverter error")
                return
            }
            append(output, state: &state)
            if status != .haveData || output.frameLength == 0 { return }
        }
    }

    private func append(_ output: AVAudioPCMBuffer, state: inout State) {
        let room = configuration.maxFrames - state.frames
        let count = min(Int(output.frameLength), max(room, 0))
        guard count > 0, let samples = output.int16ChannelData?[0] else { return }
        samples.withMemoryRebound(to: UInt8.self, capacity: count * 2) { bytes in
            state.pcm.append(bytes, count: count * 2)
        }
        state.frames += count
    }

    // MARK: Level

    /// RMS of the loudest channel (a multichannel interface may carry the voice on any input).
    static func loudestChannelRMS(_ buffer: AVAudioPCMBuffer) -> Float {
        let frames = Int(buffer.frameLength)
        guard frames > 0 else { return 0 }
        let channels = Int(buffer.format.channelCount)
        let interleaved = buffer.format.isInterleaved
        var loudest: Float = 0
        if let data = buffer.floatChannelData {
            if interleaved {
                let all = UnsafeBufferPointer(start: data[0], count: frames * channels)
                loudest = AudioLevel.rms(Array(all))
            } else {
                for channel in 0..<channels {
                    loudest = max(loudest, AudioLevel.rms(UnsafeBufferPointer(start: data[channel], count: frames)))
                }
            }
        } else if let data = buffer.int16ChannelData {
            if interleaved {
                loudest = AudioLevel.rms(UnsafeBufferPointer(start: data[0], count: frames * channels))
            } else {
                for channel in 0..<channels {
                    loudest = max(loudest, AudioLevel.rms(UnsafeBufferPointer(start: data[channel], count: frames)))
                }
            }
        }
        return loudest
    }
}

/// Hands one buffer to `AVAudioConverter`, then reports "no data for now" (or the end of the stream when
/// draining). Built outside any actor: the converter may call the block on its own terms.
private final class InputFeed: @unchecked Sendable {
    // Only touched synchronously inside `AVAudioConverter.convert`, which runs on the caller's thread.
    private var buffer: AVAudioPCMBuffer?
    private let draining: Bool

    init(buffer: AVAudioPCMBuffer?) {
        self.buffer = buffer
        draining = buffer == nil
    }

    var block: AVAudioConverterInputBlock {
        { [self] _, status in
            if let buffer {
                self.buffer = nil
                status.pointee = .haveData
                return buffer
            }
            status.pointee = draining ? .endOfStream : .noDataNow
            return nil
        }
    }
}
