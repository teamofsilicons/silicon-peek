import AVFoundation
import Foundation
import PeekCore
import Testing

@testable import PeekAudio

@Suite("Mic capture session")
struct MicCaptureSessionTests {
    @Test("48 kHz Float32 becomes a 16 kHz mono Int16 WAV of the same duration and amplitude")
    func convertsTo16kHz() throws {
        let session = MicCaptureSession()
        let tone = Signal.sine(frequency: 440, amplitude: 0.1, sampleRate: 48_000, frames: 48_000)
        for buffer in Signal.tapBuffers(tone, sampleRate: 48_000) { session.ingest(buffer) }
        let result = session.finish()

        #expect(Array(result.wav.prefix(4)) == Array("RIFF".utf8))
        #expect(Array(result.wav[22..<28]) == [1, 0, 0x80, 0x3E, 0, 0])  // mono, 16000 Hz
        let samples = Signal.int16Samples(ofWAV: result.wav)
        #expect(abs(samples.count - 16_000) <= 32, "got \(samples.count) frames")
        #expect(result.wav.count == WAVFile.headerSize + samples.count * 2)
        #expect(abs(result.durationMs - 1000) <= 2)
        // −20 dBFS peak sine → RMS −23 dBFS.
        #expect(abs(result.peakDBFS - (20 * log10(0.1 / 2.0.squareRoot()))) < 0.2)
        #expect(!result.isSilent)
        let peak = samples.dropFirst(200).map { abs(Int($0)) }.max() ?? 0
        #expect(abs(Double(peak) - 0.1 * 32768) < 0.05 * 3277)
    }

    @Test("a near-silent recording is flagged as nothing heard (peak RMS below −50 dBFS)")
    func silenceGuard() {
        let session = MicCaptureSession()
        let hiss = Signal.sine(frequency: 300, amplitude: 0.002, sampleRate: 48_000, frames: 24_000)  // ≈ −57 dBFS
        for buffer in Signal.tapBuffers(hiss, sampleRate: 48_000) { session.ingest(buffer) }
        let result = session.finish()
        #expect(result.peakDBFS < -50)
        #expect(result.isSilent)

        let loud = MicCaptureSession()
        loud.ingest(Signal.buffer([Signal.silence(frames: 1024)], sampleRate: 48_000))
        loud.ingest(Signal.buffer([Signal.sine(amplitude: 0.01, sampleRate: 48_000, frames: 1024)], sampleRate: 48_000))
        #expect(!loud.finish().isSilent)  // one audible buffer is enough (−43 dBFS)
    }

    @Test("the recording stops growing at the cap and reports it exactly once")
    func cap() {
        let session = MicCaptureSession(configuration: .init(maxDuration: .milliseconds(500)))
        #expect(session.configuration.maxFrames == 8000)
        var capHits = 0
        let tone = Signal.sine(amplitude: 0.2, sampleRate: 48_000, frames: 48_000)
        for buffer in Signal.tapBuffers(tone, sampleRate: 48_000) where session.ingest(buffer).reachedCap {
            capHits += 1
        }
        let result = session.finish()
        #expect(capHits == 1)
        #expect(Signal.int16Samples(ofWAV: result.wav).count == 8000)
        #expect(result.durationMs == 500)
        #expect(MicCaptureSession.Configuration().maxFrames == 120 * 16_000)
    }

    @Test("stereo 44.1 kHz input is downmixed and resampled; the level uses the loudest channel")
    func stereo() {
        let session = MicCaptureSession()
        let left = Signal.silence(frames: 44_100)
        let right = Signal.sine(amplitude: 0.3, sampleRate: 44_100, frames: 44_100)
        for start in stride(from: 0, to: 44_100, by: 1024) {
            let end = min(start + 1024, 44_100)
            session.ingest(Signal.buffer([Array(left[start..<end]), Array(right[start..<end])], sampleRate: 44_100))
        }
        let result = session.finish()
        #expect(abs(Signal.int16Samples(ofWAV: result.wav).count - 16_000) <= 32)
        #expect(abs(result.peakDBFS - 20 * log10(0.3 / 2.0.squareRoot())) < 0.3)
    }

    @Test("the level rises with a 30 ms attack, falls with a 150 ms release, and stops after finish")
    func level() {
        let session = MicCaptureSession()
        #expect(session.level == 0)
        let loud = Signal.buffer([Signal.sine(amplitude: 1, sampleRate: 48_000, frames: 1440)], sampleRate: 48_000)
        let first = session.ingest(loud)  // 30 ms
        let full = AudioLevel.level(rms: Float(1 / 2.0.squareRoot()))
        #expect(abs(first.bufferLevel - full) < 0.01)
        #expect(abs(first.level - full * (1 - exp(-1))) < 0.01)
        for _ in 0..<20 { session.ingest(loud) }
        #expect(abs(session.level - full) < 0.01)
        let quiet = session.ingest(Signal.buffer([Signal.silence(frames: 7200)], sampleRate: 48_000))  // 150 ms
        #expect(abs(quiet.level - full * exp(-1)) < 0.01)
        _ = session.finish()
        #expect(session.level == 0)
        #expect(session.ingest(loud) == .init(level: 0, bufferLevel: 0, reachedCap: false))
        #expect(abs(session.frameCount - session.finish().durationMs * 16) < 16)
    }

    @Test("a mid-recording format change keeps both parts")
    func formatChange() {
        let session = MicCaptureSession()
        for buffer in Signal.tapBuffers(Signal.sine(amplitude: 0.2, sampleRate: 48_000, frames: 24_000), sampleRate: 48_000) {
            session.ingest(buffer)
        }
        for buffer in Signal.tapBuffers(Signal.sine(amplitude: 0.2, sampleRate: 44_100, frames: 22_050), sampleRate: 44_100) {
            session.ingest(buffer)
        }
        let frames = Signal.int16Samples(ofWAV: session.finish().wav).count
        #expect(abs(frames - 16_000) <= 64, "got \(frames)")
        #expect(session.conversionError == nil)
    }
}

@Suite("MicRecorder")
@MainActor
struct MicRecorderTests {
    @Test("no permission: start refuses with where to fix it; requestPermission asks only when undetermined")
    func permission() async {
        let denied = FakeMicPermission(.denied)
        let recorder = MicRecorder(permissions: denied, makeDevice: { FakeMicDevice() })
        #expect(recorder.permission == .denied)
        #expect(await recorder.requestPermission() == false)
        #expect(denied.requests == 0)
        #expect {
            try recorder.start()
        } throws: { error in
            (error as? MicRecordingError)?.description.contains("System Settings › Privacy & Security › Microphone")
                == true
        }
        #expect(!recorder.isRecording)

        let undetermined = FakeMicPermission(.undetermined)
        let asking = MicRecorder(permissions: undetermined, makeDevice: { FakeMicDevice() })
        #expect(throws: MicRecordingError.self) { try asking.start() }
        #expect(await asking.requestPermission())
        #expect(undetermined.requests == 1)
        #expect(asking.permission == .granted)
    }

    @Test("record, watch the level, stop: a WAV with the peak, and the level back at 0")
    func recordAndStop() async throws {
        let device = FakeMicDevice()
        let recorder = MicRecorder(permissions: FakeMicPermission(.granted), makeDevice: { device })
        var levels: [Double] = []
        recorder.onLevel = { levels.append($0) }
        try recorder.start()
        #expect(recorder.isRecording)
        #expect(device.starts == 1)
        #expect(throws: MicRecordingError.self) { try recorder.start() }

        let tone = Signal.sine(amplitude: 0.25, sampleRate: 48_000, frames: 48_000)
        await device.deliver(Signal.tapBuffers(tone, sampleRate: 48_000))
        #expect(await waitUntil { levels.count == 47 })
        #expect(recorder.level > 0.5)
        #expect(recorder.recentLevels.count == 47)

        let result = try await recorder.stop()
        #expect(!recorder.isRecording)
        #expect(recorder.level == 0)
        #expect(device.stops == 1)
        #expect(abs(result.durationMs - 1000) <= 3)
        #expect(!result.isSilent)
        #expect(result.wav.count <= FrameLimits.maxWAVBytes)
        await #expect(throws: MicRecordingError.self) { try await recorder.stop() }
    }

    @Test("buffers that arrive after stop or cancel are ignored")
    func lateBuffers() async throws {
        let device = FakeMicDevice()
        let recorder = MicRecorder(permissions: FakeMicPermission(.granted), makeDevice: { device })
        var levels = 0
        recorder.onLevel = { _ in levels += 1 }
        try recorder.start()
        let handler = try #require(device.handler)
        recorder.cancel()
        #expect(!recorder.isRecording)
        let box = BufferBox(Signal.buffer([Signal.sine(amplitude: 0.5, sampleRate: 48_000, frames: 1024)], sampleRate: 48_000))
        await Task.detached { handler(box.buffer) }.value
        try await Task.sleep(for: .milliseconds(20))
        #expect(levels == 0)
        #expect(recorder.level == 0)
        recorder.cancel()
    }

    @Test("the 120 s cap stops the recording by itself and hands over the result")
    func autoStop() async throws {
        let device = FakeMicDevice()
        let recorder = MicRecorder(permissions: FakeMicPermission(.granted),
                                   configuration: .init(maxDuration: .milliseconds(100)), makeDevice: { device })
        var autoStopped: [MicRecordingResult] = []
        recorder.onAutoStop = { autoStopped.append($0) }
        try recorder.start()
        await device.deliver(Signal.tapBuffers(Signal.sine(amplitude: 0.3, sampleRate: 48_000, frames: 14_400),
                                               sampleRate: 48_000))
        #expect(await waitUntil { autoStopped.count == 1 })
        #expect(!recorder.isRecording)
        #expect(autoStopped.first?.durationMs == 100)
        #expect(device.stops == 1)
        try await Task.sleep(for: .milliseconds(20))
        #expect(autoStopped.count == 1)
    }

    @Test("a vanished device delivers what was recorded and says why")
    func interruption() async throws {
        let device = FakeMicDevice()
        let recorder = MicRecorder(permissions: FakeMicPermission(.granted), makeDevice: { device })
        var interruptions: [MicRecordingError] = []
        var results: [MicRecordingResult] = []
        recorder.onInterrupted = { interruptions.append($0) }
        recorder.onAutoStop = { results.append($0) }
        try recorder.start()
        await device.deliver(Signal.tapBuffers(Signal.sine(amplitude: 0.3, sampleRate: 48_000, frames: 4800),
                                               sampleRate: 48_000))
        #expect(await waitUntil { recorder.recentLevels.count == 5 })
        device.onInterrupted?(MicRecordingError("the USB microphone was unplugged"))
        #expect(interruptions.map(\.description) == ["the USB microphone was unplugged"])
        #expect(results.count == 1)
        #expect(abs((results.first?.durationMs ?? 0) - 100) <= 3)
        #expect(!recorder.isRecording)
    }

    @Test("a device that cannot start leaves the recorder idle with the device's message")
    func deviceStartFailure() {
        let device = FakeMicDevice()
        device.startError = MicRecordingError("no microphone is available")
        let recorder = MicRecorder(permissions: FakeMicPermission(.granted), makeDevice: { device })
        #expect(throws: MicRecordingError("no microphone is available")) { try recorder.start() }
        #expect(!recorder.isRecording)
        #expect(recorder.level == 0)
    }
}
