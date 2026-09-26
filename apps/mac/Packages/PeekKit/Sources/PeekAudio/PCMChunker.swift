import Accelerate
import Foundation

/// Turns peekd's `tts.chunk` bytes (signed 16-bit little-endian mono, BLUEPRINT §1.6) into Float32 buffers
/// of 20–50 ms for `AVAudioPlayerNode` (§8.7).
///
/// Network chunks can split a sample in half, so an odd trailing byte is carried into the next chunk.
/// Every buffer returned by ``append(_:)`` holds between ``minFrames`` (20 ms) and ``maxFrames`` (50 ms);
/// fewer than 20 ms are held back until more audio arrives or ``finish()`` flushes them.
public struct PCMChunker: Sendable {
    /// A buffer ready to schedule: `samples` start at `startFrame` of the stream.
    public struct Segment: Sendable, Equatable {
        public var startFrame: Int
        public var samples: [Float]

        public init(startFrame: Int, samples: [Float]) {
            self.startFrame = startFrame
            self.samples = samples
        }

        public var frameCount: Int { samples.count }
        public var endFrame: Int { startFrame + samples.count }
    }

    public let sampleRate: Int
    /// 20 ms: the smallest buffer handed out before the end of the stream.
    public let minFrames: Int
    /// 50 ms, exclusive: no buffer is this long or longer.
    public let maxFrames: Int
    /// 40 ms: the size used when a long run of audio is cut up.
    public let preferredFrames: Int

    private var carry: UInt8?
    private var pending: [Float] = []
    /// Frames decoded so far (handed out or pending).
    public private(set) var framesDecoded = 0
    /// Frames handed out in segments so far.
    public private(set) var framesEmitted = 0

    public init(sampleRate: Int = 24_000) {
        precondition(sampleRate >= 1000, "PCMChunker needs a real sample rate, got \(sampleRate)")
        self.sampleRate = sampleRate
        minFrames = sampleRate / 50
        maxFrames = sampleRate / 20
        preferredFrames = sampleRate / 25
    }

    /// True when half a sample is waiting for its second byte.
    public var hasCarryByte: Bool { carry != nil }
    /// Decoded frames not yet handed out.
    public var pendingFrames: Int { pending.count }

    /// Decodes `bytes` and returns every complete 20–50 ms buffer.
    public mutating func append(_ bytes: Data) -> [Segment] {
        guard !bytes.isEmpty else { return [] }
        var joined = Data(capacity: bytes.count + 1)
        if let carry { joined.append(carry) }
        joined.append(bytes)
        let usable = joined.count - joined.count % 2
        carry = usable < joined.count ? joined[joined.startIndex + usable] : nil
        if usable > 0 {
            pending.append(contentsOf: Self.floats(fromS16LE: joined.prefix(usable)))
            framesDecoded += usable / 2
        }
        return cut(flushAll: false)
    }

    /// Flushes whatever is pending as a final (possibly short) buffer. A dangling half sample is dropped.
    public mutating func finish() -> [Segment] {
        carry = nil
        return cut(flushAll: true)
    }

    private mutating func cut(flushAll: Bool) -> [Segment] {
        var segments: [Segment] = []
        var offset = 0
        while pending.count - offset >= maxFrames {
            segments.append(makeSegment(pending[offset..<offset + preferredFrames]))
            offset += preferredFrames
        }
        let rest = pending.count - offset
        if rest > 0, rest >= minFrames || flushAll {
            segments.append(makeSegment(pending[offset...]))
            offset = pending.count
        }
        pending.removeFirst(offset)
        return segments
    }

    private mutating func makeSegment(_ slice: ArraySlice<Float>) -> Segment {
        let segment = Segment(startFrame: framesEmitted, samples: Array(slice))
        framesEmitted += slice.count
        return segment
    }

    /// s16le bytes (even count) → Float32 in −1…1, scaled by 1/32768 (notes/speech §4.2).
    public static func floats(fromS16LE bytes: Data) -> [Float] {
        let count = bytes.count / 2
        guard count > 0 else { return [] }
        var ints = [Int16](repeating: 0, count: count)
        ints.withUnsafeMutableBytes { destination in
            _ = bytes.copyBytes(to: destination, count: count * 2)
        }
        // The wire is little-endian; this is a no-op on every Mac (arm64 and x86_64 are little-endian).
        for index in ints.indices { ints[index] = Int16(littleEndian: ints[index]) }
        var floats = [Float](repeating: 0, count: count)
        vDSP.convertElements(of: ints, to: &floats)
        vDSP.multiply(1 / 32768, floats, result: &floats)
        return floats
    }
}
