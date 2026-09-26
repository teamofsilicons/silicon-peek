import Foundation
import PeekCore
import Testing

@testable import PeekAudio

@Suite("SpeechPlayer")
@MainActor
struct SpeechPlayerTests {
    let output = FakeSpeechOutput()
    let clock = ManualClock()
    let player: SpeechPlayer
    var finished: [SpeechFinished] { recorder.finished }
    var failed: [(String, IPCErrorBody)] { recorder.failed }
    let recorder = Recorder()

    @MainActor
    final class Recorder {
        var finished: [SpeechFinished] = []
        var failed: [(String, IPCErrorBody)] = []
    }

    init() {
        let clock = self.clock
        player = SpeechPlayer(output: output, configuration: .init(startTimeout: .seconds(60)), now: { clock.now })
        let recorder = self.recorder
        player.onFinished = { recorder.finished.append($0) }
        player.onFailed = { recorder.failed.append(($0, $1)) }
    }

    func begin(_ sendID: String = "snd_1", estFrames: Int? = nil) {
        player.handle(.begin(TTSBegin(sendID: sendID, estFrames: estFrames)))
    }

    func chunk(_ samples: [Float], _ sendID: String = "snd_1", seq: Int = 0) {
        player.handle(.chunk(TTSChunk(sendID: sendID, seq: seq, pcm: Signal.s16le(samples))))
    }

    func end(_ sendID: String = "snd_1", frames: Int) {
        player.handle(.end(TTSEnd(sendID: sendID, totalFrames: frames)))
    }

    @Test("playback starts only once ~100 ms is buffered, in 20–50 ms buffers")
    func startsAfterBuffering() throws {
        begin()
        chunk(Signal.silence(frames: 1200))  // 50 ms
        let voice = try #require(output.voices.first)
        #expect(!voice.isPlaying)
        #expect(player.playback(for: "snd_1")?.started == false)
        chunk(Signal.silence(frames: 1300))  // now 104 ms
        #expect(voice.isPlaying)
        #expect(player.playback(for: "snd_1")?.started == true)
        for buffer in voice.scheduled { #expect((480..<1200).contains(buffer.samples.count)) }
        #expect(output.requestedRates == [24_000])
        #expect(output.voices.count == 1)
    }

    @Test("a short clip starts at tts.end even with less than 100 ms")
    func shortClipStartsAtEnd() {
        begin()
        chunk(Signal.silence(frames: 600))
        #expect(!output.voice.isPlaying)
        end(frames: 600)
        #expect(output.voice.isPlaying)
    }

    @Test("a slow stream starts after the start timeout")
    func startTimeout() async {
        let player = SpeechPlayer(output: output, configuration: .init(startTimeout: .milliseconds(10)))
        player.handle(.begin(TTSBegin(sendID: "snd_slow")))
        player.handle(.chunk(TTSChunk(sendID: "snd_slow", seq: 0, pcm: Signal.s16le(Signal.silence(frames: 960)))))
        #expect(!output.voice.isPlaying)
        #expect(await waitUntil { output.voice.isPlaying })
    }

    @Test("level follows the RMS of the frame being heard, smoothed")
    func levelFollowsPlayedFrame() throws {
        begin()
        chunk(Signal.silence(frames: 2400) + Signal.sine(amplitude: 0.5, sampleRate: 24_000, frames: 7200))
        let voice = output.voice
        let loud = AudioLevel.level(rms: Float(0.5 / 2.0.squareRoot()))

        voice.renderedFrames = 1200  // in the silence
        _ = player.playback(for: "snd_1")
        clock.advance(0.5)
        #expect(player.playback(for: "snd_1")?.level == 0)

        voice.renderedFrames = 4800  // in the sine
        clock.advance(0.030)
        let rising = try #require(player.playback(for: "snd_1")?.level)
        #expect(abs(rising - loud * (1 - exp(-1))) < 0.02)
        clock.advance(0.5)
        #expect(abs((player.playback(for: "snd_1")?.level ?? 0) - loud) < 0.02)

        // Calling twice in the same instant changes nothing.
        let settled = player.playback(for: "snd_1")?.level
        #expect(player.playback(for: "snd_1")?.level == settled)
    }

    @Test("progress: estimate before the end, exact after, monotonic, ≤ 0.99 until the last buffer is heard")
    func progressAndDone() throws {
        begin(estFrames: 48_000)
        chunk(Signal.silence(frames: 12_000))
        let voice = output.voice
        voice.renderedFrames = 6_000
        #expect(player.playback(for: "snd_1")?.progress == 0.125)
        chunk(Signal.silence(frames: 60_000))  // 72 000 scheduled, more than the estimate
        #expect(player.playback(for: "snd_1")?.progress == 0.125)  // raw 6000/72000 would go backwards
        voice.renderedFrames = 36_000
        end(frames: 72_000)
        let middle = try #require(player.playback(for: "snd_1"))
        #expect(middle.progress == 0.5)
        #expect(middle.totalMs == 3000)
        #expect(middle.playedMs == 1500)
        #expect(!middle.done)

        voice.renderedFrames = 72_000
        #expect(player.playback(for: "snd_1")?.progress == 0.99)
        voice.playBack(voice.scheduled.count - 1)
        #expect(finished.isEmpty)
        #expect(player.playback(for: "snd_1")?.done == false)
        voice.playBack(1)
        #expect(finished == [SpeechFinished(sendID: "snd_1", stoppedByUser: false, playedMs: 3000, totalMs: 3000)])
        let done = try #require(player.playback(for: "snd_1"))
        #expect(done.done)
        #expect(done.progress == 1)
        #expect(voice.invalidated)
        #expect(player.activeSendIDs.isEmpty)
    }

    @Test("when everything was heard before tts.end, done fires at tts.end (or with the flushed tail)")
    func doneAtEnd() {
        begin()
        chunk(Signal.silence(frames: 4800))
        output.voice.playBackAll()
        #expect(finished.isEmpty)
        end(frames: 4800)
        #expect(finished.map(\.totalMs) == [200])

        begin("snd_2")
        chunk(Signal.silence(frames: 4080), "snd_2")  // 4 × 960 scheduled, a 240-frame tail held back
        #expect(output.voice.scheduledFrames == 3840)
        output.voice.playBackAll()
        end("snd_2", frames: 4080)
        #expect(finished.count == 1)
        output.voice.playBackAll()
        #expect(finished.last == SpeechFinished(sendID: "snd_2", stoppedByUser: false, playedMs: 170, totalMs: 170))
    }

    @Test("stop: silence at once, report stopped_by_user, ignore late chunks and stale completions")
    func stopByUser() throws {
        begin(estFrames: 48_000)
        chunk(Signal.silence(frames: 24_000))
        let voice = output.voice
        voice.renderedFrames = 12_000
        player.stop(sendID: "snd_1")
        #expect(voice.stopped)
        #expect(voice.invalidated)
        #expect(finished == [SpeechFinished(sendID: "snd_1", stoppedByUser: true, playedMs: 500, totalMs: 2000)])
        chunk(Signal.silence(frames: 24_000))
        voice.playBackAll()
        end(frames: 48_000)
        player.stop(sendID: "snd_1")
        #expect(finished.count == 1)
        #expect(output.voices.count == 1)
        let state = try #require(player.playback(for: "snd_1"))
        #expect(state.done)
        #expect(state.progress == 0.25)
    }

    @Test("a failure before any audio shows the text instead (once); after audio started it plays out")
    func failures() {
        let error = IPCErrorBody(code: "speech_failed", message: "Deepgram answered 503", retryable: true)
        begin()
        chunk(Signal.silence(frames: 1200))
        player.handle(.failure(TTSErrorEvent(sendID: "snd_1", error: error)))
        #expect(failed.map(\.0) == ["snd_1"])
        #expect(failed.first?.1 == error)
        #expect(player.playback(for: "snd_1") == nil)
        #expect(output.voice.invalidated)
        player.handle(.failure(TTSErrorEvent(sendID: "snd_1", error: error)))
        chunk(Signal.silence(frames: 1200))
        #expect(failed.count == 1)

        player.handle(.failure(TTSErrorEvent(sendID: "snd_never", error: error)))
        #expect(failed.map(\.0) == ["snd_1", "snd_never"])

        begin("snd_2")
        chunk(Signal.silence(frames: 4800), "snd_2")
        #expect(output.voice.isPlaying)
        player.handle(.failure(TTSErrorEvent(sendID: "snd_2", error: error)))
        #expect(failed.count == 2)
        output.voice.playBackAll()
        #expect(finished.map(\.sendID) == ["snd_2"])
        #expect(finished.first?.stoppedByUser == false)
    }

    @Test("an unsupported format, an empty stream or a broken output device fall back to the text")
    func fallbacks() {
        player.handle(.begin(TTSBegin(sendID: "snd_mp3", format: "mp3")))
        #expect(failed.last?.1.code == "speech_format_unsupported")
        chunk(Signal.silence(frames: 4800), "snd_mp3")
        #expect(output.voices.isEmpty)

        begin("snd_empty")
        end("snd_empty", frames: 0)
        #expect(failed.last?.1.code == "speech_empty")

        output.failure = SpeechOutputError("no output device")
        begin("snd_dead")
        chunk(Signal.silence(frames: 4800), "snd_dead")
        #expect(failed.last?.0 == "snd_dead")
        #expect(failed.last?.1.code == "speech_output_unavailable")
        #expect(failed.last?.1.message == "no output device")
        #expect(failed.count == 3)
        #expect(finished.isEmpty)
    }

    @Test("chunks without tts.begin or after tts.end are ignored; tts.begin resets")
    func streamHygiene() {
        chunk(Signal.silence(frames: 4800), "snd_orphan")
        #expect(output.voices.isEmpty)
        #expect(player.playback(for: "snd_orphan") == nil)

        begin()
        chunk(Signal.silence(frames: 4800))
        let first = output.voice
        begin()  // a retry by peekd before anything played
        #expect(first.invalidated)
        #expect(player.playback(for: "snd_1")?.started == false)
        chunk(Signal.silence(frames: 2400))
        end(frames: 2400)
        chunk(Signal.silence(frames: 2400))
        #expect(output.voice.scheduledFrames == 2400)
        first.playBackAll()  // stale generation
        #expect(finished.isEmpty)
        output.voice.playBackAll()
        #expect(finished.map(\.totalMs) == [100])
    }

    @Test("an odd byte split across chunks keeps every sample")
    func oddChunks() {
        begin()
        let bytes = Signal.s16le(Signal.silence(frames: 2400))
        player.handle(.chunk(TTSChunk(sendID: "snd_1", seq: 0, pcm: bytes.prefix(2401))))
        player.handle(.chunk(TTSChunk(sendID: "snd_1", seq: 1, pcm: bytes.suffix(2399))))
        end(frames: 2400)
        #expect(output.voice.scheduledFrames == 2400)
    }

    @Test("the device latency is subtracted from the rendered position")
    func latency() {
        output.presentationLatency = 0.05
        begin(estFrames: 48_000)
        chunk(Signal.silence(frames: 24_000))
        output.voice.renderedFrames = 12_000
        #expect(player.playback(for: "snd_1")?.playedMs == 450)
    }

    @Test("a stream that ran dry re-anchors its clock when audio resumes")
    func reanchor() {
        begin(estFrames: 48_000)
        chunk(Signal.silence(frames: 2400))
        let voice = output.voice
        voice.playBackAll()  // rendered 2400
        voice.renderedFrames = 7200  // 200 ms of silence rendered while starved
        chunk(Signal.silence(frames: 2400))
        voice.renderedFrames = 8400
        #expect(player.playback(for: "snd_1")?.playedMs == 150)  // 2400 + 1200 frames, not 8400
    }

    @Test("an output device change rebuilds the engine and resumes from the frame heard")
    func configurationChange() throws {
        begin()
        chunk(Signal.sine(amplitude: 0.3, sampleRate: 24_000, frames: 24_000))
        end(frames: 24_000)
        let old = output.voice
        old.playBack(6)  // 6 × 960 frames heard
        old.renderedFrames = 6000
        _ = player.playback(for: "snd_1")  // drawings sample every frame; the last position is remembered
        output.changeConfiguration()
        #expect(output.resets == 1)
        #expect(output.voices.count == 2)
        #expect(old.invalidated)
        let fresh = output.voice
        #expect(fresh.isPlaying)
        #expect(fresh.scheduledFrames == 24_000 - 6000)
        old.playBackAll()  // stale completions from the dead voice
        #expect(finished.isEmpty)
        fresh.renderedFrames = 2400
        #expect(player.playback(for: "snd_1")?.playedMs == 350)
        fresh.playBackAll()
        #expect(finished == [SpeechFinished(sendID: "snd_1", stoppedByUser: false, playedMs: 1000, totalMs: 1000)])
    }

    @Test("the text length estimates progress when peekd sent no est_frames")
    func textEstimate() {
        player.expectSpeech(sendID: "snd_1", characterCount: 28)  // 2 s at 14 chars/s
        begin()
        chunk(Signal.silence(frames: 12_000))
        output.voice.renderedFrames = 12_000
        #expect(player.playback(for: "snd_1")?.progress == 0.25)
    }

    @Test("stopAll stops every active stream")
    func stopAll() {
        begin("snd_a")
        chunk(Signal.silence(frames: 4800), "snd_a")
        begin("snd_b")
        chunk(Signal.silence(frames: 4800), "snd_b")
        #expect(player.activeSendIDs == ["snd_a", "snd_b"])
        player.stopAll()
        #expect(Set(finished.map(\.sendID)) == ["snd_a", "snd_b"])
        #expect(finished.allSatisfy { $0.stoppedByUser })
        #expect(player.activeSendIDs.isEmpty)
    }

    @Test("a player that stops rendering ends the stream early instead of hanging the bubble")
    func stalledPlayerEndsEarly() async {
        let player = SpeechPlayer(output: output, configuration: .init(stallTimeout: .milliseconds(50),
                                                                       watchdogInterval: .milliseconds(5)),
                                  now: { ProcessInfo.processInfo.systemUptime })
        var ended: [SpeechFinished] = []
        player.onFinished = { ended.append($0) }
        player.handle(.begin(TTSBegin(sendID: "snd_stall", estFrames: 24_000)))
        player.handle(.chunk(TTSChunk(sendID: "snd_stall", seq: 0, pcm: Signal.s16le(Signal.silence(frames: 4800)))))
        output.voice.renderedFrames = 1200
        _ = player.playback(for: "snd_stall")
        output.voice.renderedFrames = nil  // the engine died without telling anyone
        #expect(await waitUntil { ended.count == 1 })
        #expect(ended.first == SpeechFinished(sendID: "snd_stall", stoppedByUser: false, playedMs: 50, totalMs: 1000))
    }
}
