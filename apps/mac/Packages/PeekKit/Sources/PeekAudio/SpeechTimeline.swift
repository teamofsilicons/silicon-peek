import Foundation

/// The loudness of a speech stream over time, indexed by stream frame (BLUEPRINT §0.1 item 2, §8.7).
///
/// Each scheduled buffer is measured as it is scheduled, in windows of ``windowFrames`` (10 ms by default),
/// so looking up the frame the player is on gives the loudness of what is audible, not of what was downloaded.
public struct LevelTimeline: Sendable {
    public let windowFrames: Int
    private var starts: [Int] = []
    private var levels: [Double] = []
    /// One past the last measured frame.
    public private(set) var endFrame = 0

    public init(windowFrames: Int) {
        precondition(windowFrames > 0, "LevelTimeline needs a positive window, got \(windowFrames)")
        self.windowFrames = windowFrames
    }

    public var isEmpty: Bool { starts.isEmpty }
    public var windowCount: Int { starts.count }

    /// Measures `samples`, which start at `startFrame`. Buffers must be appended in stream order.
    public mutating func append(_ samples: [Float], startFrame: Int) {
        guard !samples.isEmpty else { return }
        precondition(startFrame >= endFrame, "LevelTimeline buffers must be appended in order")
        samples.withUnsafeBufferPointer { buffer in
            var offset = 0
            while offset < buffer.count {
                let count = min(windowFrames, buffer.count - offset)
                let window = UnsafeBufferPointer(rebasing: buffer[offset..<offset + count])
                starts.append(startFrame + offset)
                levels.append(AudioLevel.level(rms: AudioLevel.rms(window)))
                offset += count
            }
        }
        endFrame = startFrame + samples.count
    }

    /// The level of the window containing `frame`; 0 before the first and after the last measured frame.
    public func level(atFrame frame: Int) -> Double {
        guard frame >= 0, frame < endFrame, let first = starts.first, frame >= first else { return 0 }
        var low = 0
        var high = starts.count - 1
        while low < high {
            let mid = (low + high + 1) / 2
            if starts[mid] <= frame { low = mid } else { high = mid - 1 }
        }
        return levels[low]
    }

    public mutating func removeAll() {
        starts.removeAll()
        levels.removeAll()
        endFrame = 0
    }
}

/// `speech.progress` (BLUEPRINT §8.7): played ÷ total, where total is exact after `tts.end` and
/// `max(scheduled, estimate)` before it. The value never decreases and stays ≤ 0.99 until playback is done.
public struct SpeechProgress: Sendable, Equatable {
    /// The ceiling before the final buffer has been heard.
    public static let ceilingBeforeDone = 0.99

    public private(set) var value: Double = 0

    public init() {}

    /// - Parameters:
    ///   - played: frames heard so far.
    ///   - scheduled: frames handed to the player so far.
    ///   - estimate: the expected length before `tts.end` (from `est_frames` or the text length), if any.
    ///   - total: the exact length once the stream ended.
    ///   - done: the final buffer has been played back.
    @discardableResult
    public mutating func update(played: Int, scheduled: Int, estimate: Int?, total: Int?, done: Bool) -> Double {
        if done {
            value = 1
            return value
        }
        let denominator = total ?? max(scheduled, estimate ?? 0)
        let raw = denominator > 0 ? Double(max(played, 0)) / Double(denominator) : 0
        value = min(max(value, raw), Self.ceilingBeforeDone)
        return value
    }
}
