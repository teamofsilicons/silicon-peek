import Foundation

/// Reads the end of a log file (peekd.log for Settings › Diagnostics) without loading all of it.
///
/// peekd rotates its log at 10 MB (§1.7), so the reader seeks to the last `maxBytes`, drops the
/// partial first line, and keeps at most `maxLines` lines. Invalid UTF-8 is replaced, never fatal.
public enum LogTailReader {
    public struct Tail: Equatable, Sendable {
        public var lines: [String]
        /// The file's size in bytes when it was read.
        public var fileSize: UInt64
        /// True when the file holds more than what `lines` shows.
        public var truncated: Bool

        public var text: String { lines.joined(separator: "\n") }
    }

    public enum Failure: Error, Equatable, Sendable, CustomStringConvertible {
        case missing(path: String)
        case unreadable(path: String, reason: String)

        public var description: String {
            switch self {
            case .missing(let path):
                "\(path) does not exist yet. peekd creates it when it starts; if Peek shows \"not connected\", "
                    + "peekd has not started (see Settings › Startup)."
            case .unreadable(let path, let reason):
                "cannot read \(path): \(reason)"
            }
        }
    }

    public static let defaultMaxBytes = 128 * 1024
    public static let defaultMaxLines = 300

    public static func read(_ url: URL, maxBytes: Int = defaultMaxBytes, maxLines: Int = defaultMaxLines)
        throws(Failure) -> Tail
    {
        precondition(maxBytes > 0 && maxLines > 0, "maxBytes and maxLines must be positive")
        let handle: FileHandle
        do {
            handle = try FileHandle(forReadingFrom: url)
        } catch let error as CocoaError where error.code == .fileReadNoSuchFile || error.code == .fileNoSuchFile {
            throw .missing(path: url.path)
        } catch {
            if !FileManager.default.fileExists(atPath: url.path) { throw .missing(path: url.path) }
            throw .unreadable(path: url.path, reason: error.localizedDescription)
        }
        defer { try? handle.close() }

        let size: UInt64
        let data: Data
        do {
            size = try handle.seekToEnd()
            let start = size > UInt64(maxBytes) ? size - UInt64(maxBytes) : 0
            try handle.seek(toOffset: start)
            data = try handle.readToEnd() ?? Data()
        } catch {
            throw .unreadable(path: url.path, reason: error.localizedDescription)
        }

        var bytes = data[...]
        var truncated = size > UInt64(maxBytes)
        if truncated, let newline = bytes.firstIndex(of: 0x0A) {
            // The first line is cut in the middle: start after it.
            bytes = bytes[bytes.index(after: newline)...]
        }
        var lines = String(decoding: bytes, as: UTF8.self)
            // "\r\n" is a single Character in Swift, so match it explicitly.
            .split(omittingEmptySubsequences: false, whereSeparator: { $0 == "\n" || $0 == "\r\n" })
            .map { $0.hasSuffix("\r") ? String($0.dropLast()) : String($0) }
        if lines.last == "" { lines.removeLast() }
        if lines.count > maxLines {
            lines.removeFirst(lines.count - maxLines)
            truncated = true
        }
        return Tail(lines: lines, fileSize: size, truncated: truncated)
    }
}
