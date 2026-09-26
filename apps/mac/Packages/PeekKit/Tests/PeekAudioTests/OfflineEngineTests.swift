import AVFoundation
import Foundation
import PeekCore
import Testing

@testable import PeekAudio

/// Runs the live `AVSpeechOutputEngine` / `AVSpeechVoice` against an `AVAudioEngine` in offline manual-rendering
/// mode: real player nodes, real scheduling and completions, no audio device. Offline rendering never reports
/// `.dataPlayedBack` (nothing is "heard"), so these tests complete on `.dataRendered`, and one test checks that the
/// player's clock-based safety net still ends a stream whose `.dataPlayedBack` never comes.
@Suite("AVAudioEngine speech output (offline render)")
@MainActor
struct OfflineEngineTests {
    @MainActor
    final class EngineBox {
        var engine: AVAudioEngine?
        let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!

        func make() -> AVAudioEngine {
            let engine = AVAudioEngine()
            do {
                try engine.enableManualRenderingMode(.offline, format: format, maximumFrameCount: 4096)
            } catch {
                Issue.record("cannot enable offline rendering: \(error)")
            }
            self.engine = engine
            return engine
        }

        /// Pulls `frames` output frames (48 kHz) through the engine.
        func render(_ frames: Int) throws {
            let engine = try #require(engine)
            let buffer = try #require(AVAudioPCMBuffer(pcmFormat: engine.manualRenderingFormat, frameCapacity: 4096))
            var left = frames
            while left > 0 {
                let count = AVAudioFrameCount(min(left, 4096))
                let status = try engine.renderOffline(count, to: buffer)
                #expect(status == .success)
                left -= Int(count)
            }
        }
    }

    @Test("a voice plays 24 kHz buffers, reports its rendered position and completes each buffer in order")
    func playsAndCompletes() async throws {
        let box = EngineBox()
        let output = AVSpeechOutputEngine(makeEngine: { box.make() }, completionCallbackType: .dataRendered)
        let voice = try output.makeVoice(sampleRate: 24_000)
        #expect(box.engine?.isRunning == true)
        #expect(voice.renderedFrames == nil)  // not playing yet

        var completed: [Int] = []
        for index in 0..<5 {
            voice.schedule(Signal.sine(amplitude: 0.5, sampleRate: 24_000, frames: 960)) { completed.append(index) }
        }
        voice.play()
        try box.render(4800)  // 100 ms
        let position = try #require(voice.renderedFrames)
        #expect(abs(position - 2400) <= 64, "rendered \(position) frames at 24 kHz")
        #expect(await waitUntil { completed.count >= 2 })
        #expect(completed.count <= 3)

        try box.render(9600)  // 200 ms more: everything has played
        #expect(await waitUntil { completed.count == 5 })
        #expect(completed == [0, 1, 2, 3, 4])

        voice.invalidate()
        #expect(box.engine?.isRunning == false)  // the last voice gone: the engine stops
        #expect(voice.renderedFrames == nil)
    }

    @Test("SpeechPlayer over the real engine: tts events in, level and done out")
    func speechPlayerEndToEnd() async throws {
        let box = EngineBox()
        var finished: [SpeechFinished] = []
        let player = SpeechPlayer(output: AVSpeechOutputEngine(makeEngine: { box.make() },
                                                               completionCallbackType: .dataRendered),
                                  configuration: .init(compensatesLatency: false))
        player.onFinished = { finished.append($0) }
        player.handle(.begin(TTSBegin(sendID: "snd_live", estFrames: 7200)))
        let tone = Signal.sine(amplitude: 0.5, sampleRate: 24_000, frames: 7200)  // 300 ms
        player.handle(.chunk(TTSChunk(sendID: "snd_live", seq: 0, pcm: Signal.s16le(Array(tone[..<3600])))))
        player.handle(.chunk(TTSChunk(sendID: "snd_live", seq: 1, pcm: Signal.s16le(Array(tone[3600...])))))
        player.handle(.end(TTSEnd(sendID: "snd_live", totalFrames: 7200)))
        #expect(player.playback(for: "snd_live")?.started == true)

        try box.render(7200)  // 150 ms at 48 kHz
        try await Task.sleep(for: .milliseconds(60))
        let middle = try #require(player.playback(for: "snd_live"))
        #expect(middle.level > 0.3)
        #expect(middle.progress > 0.3 && middle.progress < 0.99)
        #expect(!middle.done)

        try box.render(9600)
        #expect(await waitUntil { finished.count == 1 })
        #expect(finished.first == SpeechFinished(sendID: "snd_live", stoppedByUser: false, playedMs: 300, totalMs: 300))
        #expect(player.playback(for: "snd_live")?.progress == 1)
        #expect(box.engine?.isRunning == false)
    }

    @Test("a stream whose final .dataPlayedBack never arrives still finishes by the player's clock")
    func finishesByClock() async throws {
        let box = EngineBox()
        var finished: [SpeechFinished] = []
        let player = SpeechPlayer(output: AVSpeechOutputEngine(makeEngine: { box.make() }),  // .dataPlayedBack
                                  configuration: .init(completionGrace: 0.05, watchdogInterval: .milliseconds(5)))
        player.onFinished = { finished.append($0) }
        player.handle(.begin(TTSBegin(sendID: "snd_clock")))
        player.handle(.chunk(TTSChunk(sendID: "snd_clock", seq: 0, pcm: Signal.s16le(Signal.silence(frames: 4800)))))
        player.handle(.end(TTSEnd(sendID: "snd_clock", totalFrames: 4800)))
        try box.render(9600)  // exactly the audio: not yet past the grace
        try await Task.sleep(for: .milliseconds(40))
        #expect(finished.isEmpty)
        try box.render(4800)  // 100 ms more, past the 50 ms grace
        #expect(await waitUntil { finished.count == 1 })
        #expect(finished.first == SpeechFinished(sendID: "snd_clock", stoppedByUser: false, playedMs: 200, totalMs: 200))
    }
}
