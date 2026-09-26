import Foundation
import PeekCore
import Testing

@testable import PeekAudio

@Suite("Audio level maths")
struct AudioLevelTests {
    @Test("dBFS maps −50…0 onto 0…1 and clamps outside")
    func dbfsMapping() {
        #expect(AudioLevel.level(dbfs: -50) == 0)
        #expect(AudioLevel.level(dbfs: 0) == 1)
        #expect(abs(AudioLevel.level(dbfs: -25) - 0.5) < 1e-12)
        #expect(AudioLevel.level(dbfs: -80) == 0)
        #expect(AudioLevel.level(dbfs: 6) == 1)
        #expect(AudioLevel.level(dbfs: .nan) == 0)
        #expect(AudioLevel.dbfs(rms: 0) == AudioLevel.silenceDBFS)
        #expect(abs(AudioLevel.dbfs(rms: 1) - 0) < 1e-9)
        #expect(abs(AudioLevel.dbfs(rms: 0.1) - -20) < 1e-6)
    }

    @Test("the RMS of a sine is its amplitude over √2")
    func sineRMS() {
        let samples = Signal.sine(frequency: 1000, amplitude: 0.5, sampleRate: 48_000, frames: 48_000)
        #expect(abs(Double(AudioLevel.rms(samples)) - 0.5 / 2.0.squareRoot()) < 1e-3)
        #expect(AudioLevel.rms([Float]()) == 0)
        let ints: [Int16] = [16384, -16384, 16384, -16384]
        #expect(ints.withUnsafeBufferPointer { abs(AudioLevel.rms($0) - 0.5) < 1e-6 })
    }

    @Test("smoothing: 30 ms attack, 150 ms release, independent of step size, snaps to the target")
    func smoothing() {
        var smoother = LevelSmoother()
        smoother.step(toward: 1, dt: 0.030)
        #expect(abs(smoother.value - (1 - exp(-1))) < 1e-9)
        smoother.reset(to: 1)
        smoother.step(toward: 0, dt: 0.150)
        #expect(abs(smoother.value - exp(-1)) < 1e-9)

        var coarse = LevelSmoother()
        var fine = LevelSmoother()
        coarse.step(toward: 0.8, dt: 0.020)
        for _ in 0..<20 { fine.step(toward: 0.8, dt: 0.001) }
        #expect(abs(coarse.value - fine.value) < 1e-9)

        var decaying = LevelSmoother(value: 0.5)
        decaying.step(toward: 0, dt: 2)
        #expect(decaying.value == 0)
        #expect(decaying.step(toward: 1, dt: 0) == 0)
        #expect(decaying.step(toward: 1, dt: -1) == 0)
    }
}

@Suite("PCM chunking")
struct PCMChunkerTests {
    @Test("buffers are 20–50 ms and hold exactly the decoded samples")
    func bufferSizes() {
        var chunker = PCMChunker(sampleRate: 24_000)
        let samples = Signal.sine(amplitude: 0.5, sampleRate: 24_000, frames: 24_000)
        let bytes = Signal.s16le(samples)
        var segments: [PCMChunker.Segment] = []
        // Irregular network chunk sizes, many of them odd.
        var offset = 0
        for size in [1, 999, 4097, 12_345, 3, 7777, 20_000, 1] where offset < bytes.count {
            let end = min(offset + size, bytes.count)
            segments += chunker.append(bytes[offset..<end])
            offset = end
        }
        segments += chunker.append(bytes[offset...])
        for segment in segments {
            #expect(segment.frameCount >= 480 && segment.frameCount < 1200, "segment of \(segment.frameCount) frames")
        }
        segments += chunker.finish()
        #expect(segments.reduce(0) { $0 + $1.frameCount } == 24_000)
        #expect(chunker.framesEmitted == 24_000)
        var expectedStart = 0
        for segment in segments {
            #expect(segment.startFrame == expectedStart)
            expectedStart = segment.endFrame
        }
        let joined = segments.flatMap(\.samples)
        for index in stride(from: 0, to: 24_000, by: 997) {
            #expect(abs(joined[index] - samples[index]) <= 1 / 32768 + 1e-6)
        }
    }

    @Test("an odd byte is carried into the next chunk")
    func carryByte() {
        var chunker = PCMChunker(sampleRate: 24_000)
        let bytes = Signal.s16le([0.25, -0.5, 0.75])
        #expect(chunker.append(bytes.prefix(3)).isEmpty)
        #expect(chunker.hasCarryByte)
        #expect(chunker.framesDecoded == 1)
        #expect(chunker.append(bytes.suffix(3)).isEmpty)
        #expect(!chunker.hasCarryByte)
        #expect(chunker.framesDecoded == 3)
        let flushed = chunker.finish()
        #expect(flushed.count == 1)
        #expect(flushed[0].samples == [0.25, -0.5, 0.75])
    }

    @Test("Int16 extremes scale by 1/32768")
    func scaling() {
        var bytes = Data()
        for value: Int16 in [Int16.min, Int16.max, 0, 16384] {
            withUnsafeBytes(of: value.littleEndian) { bytes.append(contentsOf: $0) }
        }
        #expect(PCMChunker.floats(fromS16LE: bytes) == [-1, Float(32767) / 32768, 0, 0.5])
    }

    @Test("less than 20 ms is held back until more arrives or the stream ends; a dangling byte is dropped")
    func holdsShortTail() {
        var chunker = PCMChunker(sampleRate: 24_000)
        #expect(chunker.append(Signal.s16le(Signal.silence(frames: 479))).isEmpty)
        #expect(chunker.pendingFrames == 479)
        let one = chunker.append(Signal.s16le(Signal.silence(frames: 1)))
        #expect(one.map(\.frameCount) == [480])
        _ = chunker.append(Data([0x01]))
        #expect(chunker.hasCarryByte)
        #expect(chunker.finish().isEmpty)
        #expect(!chunker.hasCarryByte)
    }
}

@Suite("Speech level timeline and progress")
struct SpeechTimelineTests {
    @Test("the timeline gives the level of the 10 ms window containing a frame")
    func timelineLookup() {
        var timeline = LevelTimeline(windowFrames: 240)
        timeline.append(Signal.silence(frames: 480), startFrame: 0)
        timeline.append(Signal.sine(amplitude: 0.5, sampleRate: 24_000, frames: 720), startFrame: 480)
        #expect(timeline.windowCount == 5)
        #expect(timeline.endFrame == 1200)
        #expect(timeline.level(atFrame: 0) == 0)
        #expect(timeline.level(atFrame: 479) == 0)
        let loud = AudioLevel.level(rms: Float(0.5 / 2.0.squareRoot()))
        #expect(abs(timeline.level(atFrame: 480) - loud) < 0.02)
        #expect(abs(timeline.level(atFrame: 1199) - loud) < 0.02)
        #expect(timeline.level(atFrame: 1200) == 0)
        #expect(timeline.level(atFrame: -5) == 0)
    }

    @Test("before tts.end: played ÷ max(scheduled, estimate), capped at 0.99")
    func progressBeforeEnd() {
        var progress = SpeechProgress()
        #expect(progress.update(played: 0, scheduled: 0, estimate: nil, total: nil, done: false) == 0)
        #expect(progress.update(played: 12_000, scheduled: 24_000, estimate: 48_000, total: nil, done: false) == 0.25)
        #expect(progress.update(played: 30_000, scheduled: 100_000, estimate: 48_000, total: nil, done: false) == 0.3)
        #expect(progress.update(played: 100_000, scheduled: 100_000, estimate: nil, total: nil, done: false) == 0.99)
    }

    @Test("progress never goes backwards, is exact after tts.end, and reaches 1 only when done")
    func progressMonotonicAndExact() {
        var progress = SpeechProgress()
        progress.update(played: 24_000, scheduled: 48_000, estimate: nil, total: nil, done: false)
        #expect(progress.value == 0.5)
        // More audio arrived: the raw ratio drops to 0.25, the reported value holds.
        #expect(progress.update(played: 24_000, scheduled: 96_000, estimate: nil, total: nil, done: false) == 0.5)
        #expect(progress.update(played: 72_000, scheduled: 96_000, estimate: nil, total: 96_000, done: false) == 0.75)
        #expect(progress.update(played: 96_000, scheduled: 96_000, estimate: nil, total: 96_000, done: false) == 0.99)
        #expect(progress.update(played: 96_000, scheduled: 96_000, estimate: nil, total: 96_000, done: true) == 1)
    }
}

@Suite("WAV header")
struct WAVFileTests {
    @Test("the 44-byte RIFF header for 16 kHz mono 16-bit, byte for byte")
    func headerBytes() {
        let header = WAVFile.header(dataByteCount: 32_000)
        let expected: [UInt8] = [
            0x52, 0x49, 0x46, 0x46,  // "RIFF"
            0x24, 0x7D, 0x00, 0x00,  // 36 + 32000
            0x57, 0x41, 0x56, 0x45,  // "WAVE"
            0x66, 0x6D, 0x74, 0x20,  // "fmt "
            0x10, 0x00, 0x00, 0x00,  // 16
            0x01, 0x00,  // PCM
            0x01, 0x00,  // mono
            0x80, 0x3E, 0x00, 0x00,  // 16000 Hz
            0x00, 0x7D, 0x00, 0x00,  // 32000 bytes/s
            0x02, 0x00,  // block align
            0x10, 0x00,  // 16 bits
            0x64, 0x61, 0x74, 0x61,  // "data"
            0x00, 0x7D, 0x00, 0x00,  // 32000
        ]
        #expect(Array(header) == expected)
        #expect(header.count == WAVFile.headerSize)
    }

    @Test("a WAV file is the header plus the PCM; a 120 s recording fits the 4 MiB IPC limit")
    func file() {
        let pcm = Data([1, 2, 3, 4])
        let wav = WAVFile.make(pcm: pcm)
        #expect(wav.count == 48)
        #expect(Array(wav.suffix(4)) == [1, 2, 3, 4])
        #expect(Array(wav[40..<44]) == [4, 0, 0, 0])
        #expect(WAVFile.headerSize + 120 * 16_000 * 2 <= FrameLimits.maxWAVBytes)
        #expect(Array(WAVFile.header(dataByteCount: 0)[4..<8]) == [36, 0, 0, 0])
    }
}
