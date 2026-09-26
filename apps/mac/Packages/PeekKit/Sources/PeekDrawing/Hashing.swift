import CoreGraphics
import Foundation

/// A small deterministic 64-bit hash (splitmix-style mixing per word) for layer diffing (visual.md B5).
///
/// `Hasher` is randomly seeded per process; layer hashes also appear in `--dump-frame` output, where
/// stable values across runs are easier to read and compare.
struct Hash64: Hashable, Sendable {
    private(set) var value: UInt64

    init(seed: UInt64 = 0x243F_6A88_85A3_08D3) { value = seed }

    mutating func mix(_ word: UInt64) {
        var z = (value ^ word) &* 0x9E37_79B9_7F4A_7C15
        z = (z ^ (z >> 29)) &* 0xBF58_476D_1CE4_E5B9
        value = z ^ (z >> 32)
    }

    mutating func mix(_ number: Double) { mix(number.bitPattern) }
    mutating func mix(_ number: Int) { mix(UInt64(bitPattern: Int64(number))) }
    mutating func mix(_ flag: Bool) { mix(flag ? 1 as UInt64 : 2) }

    mutating func mix(_ transform: CGAffineTransform) {
        mix(Double(transform.a))
        mix(Double(transform.b))
        mix(Double(transform.c))
        mix(Double(transform.d))
        mix(Double(transform.tx))
        mix(Double(transform.ty))
    }

    mutating func mix(_ string: String) {
        var h: UInt64 = 0xcbf2_9ce4_8422_2325
        for byte in string.utf8 { h = (h ^ UInt64(byte)) &* 0x0000_0100_0000_01B3 }
        mix(h)
        mix(string.utf8.count)
    }

    mutating func mix(_ other: Hash64) { mix(other.value) }

    /// Short form for dumps, e.g. `g:7f3a91` (visual.md A10).
    func short(_ prefix: String) -> String { prefix + ":" + String(format: "%06x", value & 0xFF_FFFF) }
}

extension CGPath {
    /// Content hash of every element and point.
    var contentHash: Hash64 {
        var hash = Hash64(seed: 0x1357_9BDF_2468_ACE0)
        applyWithBlock { pointer in
            let element = pointer.pointee
            hash.mix(Int(element.type.rawValue))
            let count: Int
            switch element.type {
            case .moveToPoint, .addLineToPoint: count = 1
            case .addQuadCurveToPoint: count = 2
            case .addCurveToPoint: count = 3
            case .closeSubpath: count = 0
            @unknown default: count = 0
            }
            for index in 0..<count {
                hash.mix(Double(element.points[index].x))
                hash.mix(Double(element.points[index].y))
            }
        }
        return hash
    }
}
