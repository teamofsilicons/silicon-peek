import Accelerate
import Foundation

/// Loudness maths shared by speech playback and the microphone (BLUEPRINT §8.7, notes/speech §4.2–4.3):
/// RMS with `vDSP_rmsqv`, then dBFS, then the 0…1 level drawings see, where −50 dBFS is 0 and 0 dBFS is 1.
public enum AudioLevel {
    /// The bottom of the level scale: anything at or below −50 dBFS reads as 0.
    public static let floorDBFS = -50.0
    /// What digital silence (an RMS of 0) is reported as, so dBFS values stay finite.
    public static let silenceDBFS = -160.0

    /// Root mean square of `samples` (0 for an empty buffer).
    public static func rms(_ samples: UnsafeBufferPointer<Float>) -> Float {
        guard let base = samples.baseAddress, !samples.isEmpty else { return 0 }
        var result: Float = 0
        vDSP_rmsqv(base, 1, &result, vDSP_Length(samples.count))
        return result.isFinite ? result : 0
    }

    public static func rms(_ samples: [Float]) -> Float {
        samples.withUnsafeBufferPointer { rms($0) }
    }

    /// Root mean square of 16-bit samples, on the same full-scale reference as Float32 (32768 = 1.0).
    public static func rms(_ samples: UnsafeBufferPointer<Int16>) -> Float {
        guard !samples.isEmpty else { return 0 }
        var floats = [Float](repeating: 0, count: samples.count)
        vDSP.convertElements(of: samples, to: &floats)
        vDSP.multiply(1 / 32768, floats, result: &floats)
        return rms(floats)
    }

    /// `20·log10(rms)`, floored at ``silenceDBFS``.
    public static func dbfs(rms: Float) -> Double {
        guard rms.isFinite, rms > 0 else { return silenceDBFS }
        return max(silenceDBFS, 20 * log10(Double(rms)))
    }

    /// Maps dBFS onto 0…1: `clamp((dB + 50) / 50, 0, 1)`.
    public static func level(dbfs: Double) -> Double {
        guard dbfs.isFinite else { return 0 }
        return min(max((dbfs - floorDBFS) / -floorDBFS, 0), 1)
    }

    public static func level(rms: Float) -> Double {
        level(dbfs: dbfs(rms: rms))
    }
}

/// Attack/release smoothing of a 0…1 level. Time-based (exact exponential), so the result does not
/// depend on how often it is stepped: stepping twice by 5 ms equals stepping once by 10 ms.
public struct LevelSmoother: Sendable, Equatable {
    /// Time constant while the level rises, in seconds (BLUEPRINT §8.7: about 30 ms).
    public var attack: Double
    /// Time constant while the level falls, in seconds (about 150 ms).
    public var release: Double
    public private(set) var value: Double

    /// Values closer than this to the target snap to it, so a decaying level reaches exactly 0.
    public static let snapDistance = 0.001

    public init(attack: Double = 0.030, release: Double = 0.150, value: Double = 0) {
        self.attack = attack
        self.release = release
        self.value = value
    }

    /// Moves the value toward `target` over `dt` seconds and returns it. `dt <= 0` changes nothing.
    @discardableResult
    public mutating func step(toward target: Double, dt: Double) -> Double {
        let target = target.isFinite ? min(max(target, 0), 1) : 0
        guard dt > 0, dt.isFinite else { return value }
        let tau = target > value ? attack : release
        let coefficient = tau > 0 ? 1 - exp(-dt / tau) : 1
        value += (target - value) * coefficient
        if abs(value - target) < Self.snapDistance { value = target }
        return value
    }

    public mutating func reset(to value: Double = 0) {
        self.value = value
    }
}
