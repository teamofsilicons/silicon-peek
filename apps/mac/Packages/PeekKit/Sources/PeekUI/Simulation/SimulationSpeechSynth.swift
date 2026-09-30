import Foundation

/// Synthesizes the sample "speech" Simulation plays: a deterministic, speech-like tone sequence in
/// exactly the format peekd streams from ElevenLabs v4 (s16le, mono, 24 kHz; BLUEPRINT §8.7).
///
/// It is not intelligible speech and makes no network call. Each word becomes one to five voiced
/// syllables (a harmonic source shaped by vowel formants, with a declining pitch contour and short
/// consonant noise bursts), separated by word gaps and longer pauses at punctuation, so
/// `speech.level` rises and falls like talking and `speech.progress` tracks the text's length
/// (about 14 characters per second, the rate peekd assumes before `tts.end`).
public enum SimulationSpeechSynth {
    public static let sampleRate = 24_000

    public struct Clip: Sendable, Equatable {
        /// Little-endian signed 16-bit PCM.
        public let pcm: Data
        public var frames: Int { pcm.count / 2 }
        public var durationMs: Int { frames * 1000 / SimulationSpeechSynth.sampleRate }
    }

    /// The clip for `text`. The same text always gives the same bytes.
    public static func synthesize(_ text: String) -> Clip {
        var rng = SplitMix64(seed: fnv1a(text))
        let plan = plan(text, rng: &rng)
        let totalSeconds = plan.reduce(0) { $0 + $1.duration }
        let frameCount = max(1, Int((totalSeconds * Double(sampleRate)).rounded(.up)))
        var samples = [Double](repeating: 0, count: frameCount)

        let question = text.trimmingCharacters(in: .whitespacesAndNewlines).hasSuffix("?")
        let voicedSyllables = plan.filter { if case .syllable = $0.kind { true } else { false } }.count
        var syllableIndex = 0
        var cursor = 0
        for segment in plan {
            let length = Int((segment.duration * Double(sampleRate)).rounded())
            defer { cursor += length }
            guard case .syllable(let vowel, let stressed, let onsetNoise) = segment.kind, length > 0 else { continue }
            // Pitch declines across the utterance (≈ 205 → 150 Hz); questions rise at the end.
            let position = voicedSyllables > 1 ? Double(syllableIndex) / Double(voicedSyllables - 1) : 0
            var basePitch = 205 - 55 * position
            if question, syllableIndex >= voicedSyllables - 2 { basePitch += 45 * position }
            syllableIndex += 1
            renderSyllable(
                into: &samples, start: cursor, length: length, pitch: basePitch + (stressed ? 22 : 0), vowel: vowel,
                peak: (stressed ? 0.46 : 0.34) + rng.nextDouble() * 0.08, onsetNoise: onsetNoise, rng: &rng)
        }

        // Normalise to a −3 dBFS peak so every clip sounds equally loud.
        let peak = samples.reduce(0) { max($0, abs($1)) }
        let gain = peak > 0 ? 0.708 / peak : 0
        var pcm = Data(capacity: frameCount * 2)
        for sample in samples {
            let value = Int16(clamping: Int((sample * gain * 32767).rounded()))
            withUnsafeBytes(of: value.littleEndian) { pcm.append(contentsOf: $0) }
        }
        return Clip(pcm: pcm)
    }

    // MARK: Plan

    struct Segment {
        enum Kind {
            case silence
            case syllable(vowel: Vowel, stressed: Bool, onsetNoise: Bool)
        }

        var kind: Kind
        var duration: Double
    }

    struct Vowel {
        var f1: Double
        var f2: Double
        static let all = [
            Vowel(f1: 730, f2: 1090), Vowel(f1: 530, f2: 1840), Vowel(f1: 270, f2: 2290),
            Vowel(f1: 570, f2: 840), Vowel(f1: 300, f2: 870), Vowel(f1: 500, f2: 1500),
        ]
    }

    static func plan(_ text: String, rng: inout SplitMix64) -> [Segment] {
        var segments = [Segment(kind: .silence, duration: 0.06)]
        let words = text.split(whereSeparator: { $0.isWhitespace || $0.isNewline })
        for (wordIndex, word) in words.enumerated() {
            let count = syllableCount(word)
            for syllable in 0..<count {
                let stressed = syllable == 0 && (count > 1 || wordIndex % 3 == 0)
                segments.append(
                    Segment(
                        kind: .syllable(
                            vowel: Vowel.all[rng.nextInt(below: Vowel.all.count)], stressed: stressed,
                            onsetNoise: rng.nextDouble() < 0.6),
                        duration: 0.165 + rng.nextDouble() * 0.06 + (stressed ? 0.03 : 0)))
            }
            let pause: Double
            switch word.last {
            case "."?, "!"?, "?"?, "…"?: pause = 0.38
            case ","?, ";"?, ":"?, "—"?, "–"?: pause = 0.22
            default: pause = 0.05 + rng.nextDouble() * 0.03
            }
            segments.append(Segment(kind: .silence, duration: pause))
        }
        segments.append(Segment(kind: .silence, duration: 0.12))
        return segments
    }

    /// Vowel groups in the word's letters (1…5); digit runs count one syllable per digit (≤ 4).
    static func syllableCount(_ word: Substring) -> Int {
        let letters = word.lowercased().filter(\.isLetter)
        if letters.isEmpty {
            let digits = word.filter(\.isNumber).count
            return digits == 0 ? 0 : min(4, digits)
        }
        var groups = 0
        var inVowel = false
        for character in letters {
            let vowel = "aeiouyàáâäãåèéêëìíîïòóôöõùúûüæœ".contains(character) || !character.isASCII
            if vowel, !inVowel { groups += 1 }
            inVowel = vowel
        }
        return min(5, max(1, groups))
    }

    // MARK: Rendering

    static func renderSyllable(into samples: inout [Double], start: Int, length: Int, pitch: Double, vowel: Vowel,
                               peak: Double, onsetNoise: Bool, rng: inout SplitMix64) {
        let rate = Double(sampleRate)
        let attack = 0.022 * rate, release = 0.055 * rate
        let noiseLength = onsetNoise ? Int((0.018 + rng.nextDouble() * 0.03) * rate) : 0
        let voicedStart = min(length, noiseLength / 2)
        let maxHarmonics = 24
        var phases = [Double](repeating: rng.nextDouble() * 2 * .pi, count: maxHarmonics)
        var amplitudes = [Double](repeating: 0, count: maxHarmonics)
        var amplitudeSum = 1.0
        var previousNoise = 0.0

        for i in 0..<length where start + i < samples.count {
            let t = Double(i) / Double(max(1, length - 1))
            // A rise-fall pitch accent within the syllable plus a little vibrato.
            let f0 = pitch * (1 + 0.06 * sin(.pi * t) - 0.03 * t) + 1.5 * sin(2 * .pi * 5.5 * Double(i) / rate)
            if i % 120 == 0 {
                amplitudeSum = 0
                for h in 0..<maxHarmonics {
                    let frequency = Double(h + 1) * f0
                    guard frequency < 4_200 else {
                        amplitudes[h] = 0
                        continue
                    }
                    let shape = gaussian(frequency, vowel.f1, 110) + 0.7 * gaussian(frequency, vowel.f2, 170)
                        + 0.25 * gaussian(frequency, 2_750, 260) + 0.06
                    amplitudes[h] = shape / pow(Double(h + 1), 0.85)
                    amplitudeSum += amplitudes[h]
                }
            }
            var voiced = 0.0
            if i >= voicedStart {
                for h in 0..<maxHarmonics where amplitudes[h] > 0 {
                    phases[h] += 2 * .pi * Double(h + 1) * f0 / rate
                    if phases[h] > 2 * .pi { phases[h] -= 2 * .pi }
                    voiced += amplitudes[h] * sin(phases[h])
                }
                voiced /= max(amplitudeSum, 1e-6)
            }
            let fromStart = Double(i - voicedStart), toEnd = Double(length - i)
            let envelope = raisedCosine(min(1, max(0, fromStart / attack))) * raisedCosine(min(1, toEnd / release))
            var value = voiced * envelope * peak * (0.9 + 0.1 * sin(2 * .pi * 3 * t))
            if i < noiseLength {
                // A fricative-like burst: first-differenced white noise, faded in and out.
                let white = rng.nextDouble() * 2 - 1
                let bright = white - previousNoise
                previousNoise = white
                let n = Double(i) / Double(noiseLength)
                value += bright * 0.06 * sin(.pi * n)
            }
            samples[start + i] += value
        }
    }

    static func gaussian(_ x: Double, _ mean: Double, _ width: Double) -> Double {
        let z = (x - mean) / width
        return exp(-0.5 * z * z)
    }

    static func raisedCosine(_ x: Double) -> Double { 0.5 - 0.5 * cos(.pi * x) }

    static func fnv1a(_ text: String) -> UInt64 {
        var hash: UInt64 = 0xcbf2_9ce4_8422_2325
        for byte in text.utf8 {
            hash ^= UInt64(byte)
            hash = hash &* 0x0000_0100_0000_01b3
        }
        return hash
    }
}

/// A small deterministic PRNG (SplitMix64) so samples are identical on every run.
struct SplitMix64 {
    private var state: UInt64

    init(seed: UInt64) { state = seed }

    mutating func next() -> UInt64 {
        state &+= 0x9E37_79B9_7F4A_7C15
        var z = state
        z = (z ^ (z >> 30)) &* 0xBF58_476D_1CE4_E5B9
        z = (z ^ (z >> 27)) &* 0x94D0_49BB_1331_11EB
        return z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    mutating func nextDouble() -> Double { Double(next() >> 11) / Double(1 << 53) }

    mutating func nextInt(below bound: Int) -> Int { Int(next() % UInt64(bound)) }
}
