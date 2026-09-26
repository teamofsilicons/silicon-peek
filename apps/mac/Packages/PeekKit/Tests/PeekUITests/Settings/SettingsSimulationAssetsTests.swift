import CryptoKit
import Foundation
import ImageIO
import PeekCore
import Testing

@testable import PeekUI

@Suite("Simulation samples: speech PCM, artwork, cassette")
struct SettingsSimulationAssetsTests {
    private func samples(_ clip: SimulationSpeechSynth.Clip) -> [Int16] {
        clip.pcm.withUnsafeBytes { raw in
            (0..<clip.frames).map { Int16(littleEndian: raw.loadUnaligned(fromByteOffset: $0 * 2, as: Int16.self)) }
        }
    }

    /// RMS (0…1) of each 20 ms window.
    private func windowLevels(_ values: [Int16]) -> [Double] {
        let window = SimulationSpeechSynth.sampleRate / 50
        return stride(from: 0, to: values.count - window, by: window).map { start in
            let sum = values[start..<(start + window)].reduce(0.0) { $0 + Double($1) * Double($1) }
            return (sum / Double(window)).squareRoot() / 32768
        }
    }

    @Test("speech PCM is deterministic s16le mono 24 kHz")
    func speechIsDeterministic() {
        let text = SimulationSamples.speakOnly
        let first = SimulationSpeechSynth.synthesize(text)
        #expect(first == SimulationSpeechSynth.synthesize(text))
        #expect(first.pcm.count % 2 == 0)
        #expect(first.frames == first.pcm.count / 2)
        #expect(SimulationSpeechSynth.synthesize("Something else entirely.") != first)
    }

    @Test("speech lasts about as long as peekd's 14 characters per second estimate")
    func speechDuration() {
        for text in [SimulationSamples.speakOnly, "Now playing Neon Tide by Lumen Harbor, from 2024.", "Keep it?"] {
            let clip = SimulationSpeechSynth.synthesize(text)
            let estimate = Double(text.count) / 14
            let seconds = Double(clip.frames) / Double(SimulationSpeechSynth.sampleRate)
            #expect(seconds > estimate * 0.5 && seconds < estimate * 2.2, "\(text): \(seconds) s vs estimate \(estimate) s")
        }
    }

    @Test("speech rises and falls like talking, peaks at −3 dBFS and starts and ends in silence")
    func speechLevels() throws {
        let clip = SimulationSpeechSynth.synthesize(SimulationSamples.speakOnly)
        let values = samples(clip)
        let peak = values.map { abs(Int($0)) }.max() ?? 0
        #expect((22_500...23_300).contains(peak), "peak \(peak)")
        let levels = windowLevels(values)
        #expect(levels.filter { $0 > 0.08 }.count > levels.count / 3, "mostly voiced")
        #expect(levels.contains { $0 < 0.005 }, "with pauses between words")
        let first = try #require(levels.first)
        let last = try #require(levels.last)
        #expect(first < 0.01 && last < 0.01)
        // The 200 ms tts.chunk size the engine streams stays far below the 64 KiB frame limit.
        #expect(SimulationEngine.chunkFrames * 2 <= FrameLimits.maxTTSChunkBytes)
    }

    @Test("an utterance without letters still produces a short clip")
    func speechEdgeCases() {
        #expect(SimulationSpeechSynth.synthesize("12.4").frames > SimulationSpeechSynth.sampleRate / 10)
        #expect(SimulationSpeechSynth.synthesize("— …").frames > 0)
        #expect(SimulationSpeechSynth.syllableCount("playlist") == 2)
        #expect(SimulationSpeechSynth.syllableCount("a") == 1)
        #expect(SimulationSpeechSynth.syllableCount("rhythm") == 1)
        #expect(SimulationSpeechSynth.syllableCount("—") == 0)
    }

    @Test("every cover and option image is a PNG of the documented size with real content")
    func artwork() throws {
        func check(_ data: Data, size: Int, name: String) throws {
            let source = try #require(CGImageSourceCreateWithData(data as CFData, nil), "\(name) is not an image")
            #expect(CGImageSourceGetType(source) as String? == "public.png")
            let image = try #require(CGImageSourceCreateImageAtIndex(source, 0, nil))
            #expect(image.width == size && image.height == size, "\(name)")
            #expect(data.count > 2_000, "\(name) looks blank (\(data.count) bytes)")
        }
        for cover in SimulationArtwork.Cover.allCases {
            try check(try SimulationArtwork.png(cover), size: SimulationArtwork.coverSize, name: cover.rawValue)
            #expect(!cover.title.isEmpty && !cover.artist.isEmpty && cover.year.count == 4)
        }
        for icon in SimulationArtwork.Icon.allCases {
            try check(try SimulationArtwork.png(icon), size: SimulationArtwork.iconSize, name: icon.rawValue)
        }
    }

    @Test("materializing writes every sample once and keeps cassette.js current")
    func materialize() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("peek-sim-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: directory) }

        let paths = try SimulationAssets.materialize(into: directory)
        #expect(paths.directory == directory)
        #expect(try Data(contentsOf: paths.cassette) == SimulationCassette.data)
        let digest = SHA256.hash(data: try Data(contentsOf: paths.cassette)).map { String(format: "%02x", $0) }.joined()
        #expect(paths.cassetteSHA256 == digest)
        for cover in SimulationArtwork.Cover.allCases {
            #expect(FileManager.default.fileExists(atPath: paths.cover(cover)))
        }
        for icon in SimulationArtwork.Icon.allCases {
            #expect(FileManager.default.fileExists(atPath: paths.icon(icon)))
        }
        // Every image a preset references exists on disk.
        for preset in SimulationPreset.allCases {
            let event = try preset.scenario.makeEvent(assets: paths, sendID: "snd_x", askID: "ask_x")
            for path in (event.show?.imagePaths ?? []) + (event.ask?.imagePaths ?? []) {
                #expect(FileManager.default.fileExists(atPath: path), "\(preset.rawValue): \(path)")
            }
        }

        // A second run keeps the images and repairs a modified cassette.
        let coverPath = paths.cover(.neonTide)
        let before = try FileManager.default.attributesOfItem(atPath: coverPath)[.modificationDate] as? Date
        try Data("tampered".utf8).write(to: paths.cassette)
        _ = try SimulationAssets.materialize(into: directory)
        let after = try FileManager.default.attributesOfItem(atPath: coverPath)[.modificationDate] as? Date
        #expect(before == after)
        #expect(try Data(contentsOf: paths.cassette) == SimulationCassette.data)
    }

    @Test("the assets live under ~/Library/Caches/Peek/Simulation")
    func assetsDirectory() {
        let paths = PeekPaths(home: URL(fileURLWithPath: "/Users/someone"))
        #expect(SimulationAssets.directory(for: paths).path == "/Users/someone/Library/Caches/Peek/Simulation/v\(SimulationAssets.version)")
    }

    @Test("the cassette is the amended visual.md example with peek.log and the SIMULATION label")
    func cassette() {
        let source = SimulationCassette.source
        #expect(SimulationCassette.data.count <= FrameLimits.maxDrawingBytes)
        #expect(source.contains("peek.frame((ctx, input) =>"))
        #expect(source.contains("ctx.fillGlass(body, { style: 'clear', tint, interactive: true, rule: 'evenodd' })"))
        #expect(source.contains("input.context === 'simulation' ? 'SIMULATION' : 'SIDE A'"))
        #expect(source.contains("peek.log("))
        #expect(source.contains("if (input.mode === 'compact')"))
        // Each reel hole starts its own subpath, so the glass body has no triangular wedge (canvas arc() rule).
        #expect(source.contains("body.moveTo(x + 9, y); body.arc(x, y, 9, 0, Math.PI * 2)"))
        // BLUEPRINT §0.1 item 1: no live word or transcript data anywhere.
        #expect(!source.contains("speech.word") && !source.contains("speech?.word") && !source.contains("transcript"))
        #expect(!source.contains("'word'"))
        #expect(SimulationCassette.sha256.count == 64)
    }
}
