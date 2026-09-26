import Foundation
import OSLog
import Synchronization

/// Peek.app's log (BLUEPRINT §1.7): every line goes to the unified log (subsystem `ai.tos.peek`) and,
/// once the app has called ``PeekLog/configure(file:)``, to `ui.log` in the support directory, rotated at
/// 10 MB with 3 files kept (`ui.log`, `ui.log.1`, `ui.log.2`).
///
/// Log lines never carry audio, transcripts, typed text or tokens: callers pass only ids, states and
/// error descriptions. Tests never configure the file, so they only reach the unified log.
public struct PeekLogger: Sendable {
    public enum Level: String, Sendable, Comparable {
        case debug, info, notice, warning, error

        var rank: Int {
            switch self {
            case .debug: 0
            case .info: 1
            case .notice: 2
            case .warning: 3
            case .error: 4
            }
        }

        public static func < (lhs: Level, rhs: Level) -> Bool { lhs.rank < rhs.rank }
    }

    public static let subsystem = "ai.tos.peek"

    public let category: String
    private let logger: Logger

    public init(category: String) {
        self.category = category
        logger = Logger(subsystem: Self.subsystem, category: category)
    }

    public func debug(_ message: @autoclosure () -> String) { log(.debug, message()) }
    public func info(_ message: @autoclosure () -> String) { log(.info, message()) }
    public func notice(_ message: @autoclosure () -> String) { log(.notice, message()) }
    public func warning(_ message: @autoclosure () -> String) { log(.warning, message()) }
    public func error(_ message: @autoclosure () -> String) { log(.error, message()) }

    public func log(_ level: Level, _ message: String) {
        switch level {
        case .debug: logger.debug("\(message, privacy: .public)")
        case .info: logger.info("\(message, privacy: .public)")
        case .notice: logger.notice("\(message, privacy: .public)")
        case .warning: logger.warning("\(message, privacy: .public)")
        case .error: logger.error("\(message, privacy: .public)")
        }
        PeekLog.write(level, category: category, message)
    }
}

/// The process-wide `ui.log` sink.
public enum PeekLog {
    private static let sink = Mutex<RotatingLogFile?>(nil)

    /// Starts mirroring log lines (info and above) into `file`. Call once at launch; `nil` stops it.
    public static func configure(file: URL?, maxBytes: Int = RotatingLogFile.defaultMaxBytes,
                                 keptFiles: Int = RotatingLogFile.defaultKeptFiles) {
        let next = file.map { RotatingLogFile(url: $0, maxBytes: maxBytes, keptFiles: keptFiles) }
        sink.withLock { $0 = next }
    }

    public static var isConfigured: Bool { sink.withLock { $0 != nil } }

    static func write(_ level: PeekLogger.Level, category: String, _ message: String) {
        guard level >= .info else { return }
        sink.withLock { $0?.append(level: level, category: category, message: message) }
    }
}

/// An append-only text log rotated by size: `name` → `name.1` → … → `name.<kept-1>` (the oldest is deleted).
/// Not thread-safe on its own: ``PeekLog`` serialises every access under its lock (hence `@unchecked`).
public final class RotatingLogFile: @unchecked Sendable {
    public static let defaultMaxBytes = 10 * 1024 * 1024
    public static let defaultKeptFiles = 3

    public let url: URL
    public let maxBytes: Int
    public let keptFiles: Int
    private var handle: FileHandle?
    private var size: Int = 0
    private var failed = false

    public init(url: URL, maxBytes: Int = defaultMaxBytes, keptFiles: Int = defaultKeptFiles) {
        self.url = url
        self.maxBytes = max(1024, maxBytes)
        self.keptFiles = max(1, keptFiles)
    }

    deinit { try? handle?.close() }

    /// The rotated files, newest first (`ui.log`, `ui.log.1`, …).
    public var allFiles: [URL] {
        (0..<keptFiles).map { $0 == 0 ? url : url.deletingLastPathComponent().appendingPathComponent("\(url.lastPathComponent).\($0)") }
    }

    public func append(level: PeekLogger.Level, category: String, message: String, date: Date = Date()) {
        let line = "\(Self.timestamp(date)) \(level.rawValue.uppercased()) [\(category)] \(message.replacingOccurrences(of: "\n", with: " ⏎ "))\n"
        let data = Data(line.utf8)
        guard openIfNeeded() else { return }
        if size > 0, size + data.count > maxBytes { rotate() }
        guard let handle else { return }
        do {
            try handle.write(contentsOf: data)
            size += data.count
        } catch {
            close()
        }
    }

    private func openIfNeeded() -> Bool {
        if handle != nil { return true }
        if failed { return false }
        let fm = FileManager.default
        do {
            try fm.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true,
                                   attributes: [.posixPermissions: NSNumber(value: 0o700)])
            if !fm.fileExists(atPath: url.path) {
                guard fm.createFile(atPath: url.path, contents: nil, attributes: [.posixPermissions: NSNumber(value: 0o600)])
                else { failed = true; return false }
            }
            let handle = try FileHandle(forWritingTo: url)
            size = Int(try handle.seekToEnd())
            self.handle = handle
            return true
        } catch {
            failed = true
            return false
        }
    }

    private func rotate() {
        close()
        let fm = FileManager.default
        let files = allFiles
        try? fm.removeItem(at: files[files.count - 1])
        if files.count > 1 {
            for index in stride(from: files.count - 2, through: 0, by: -1) where fm.fileExists(atPath: files[index].path) {
                try? fm.moveItem(at: files[index], to: files[index + 1])
            }
        } else {
            try? fm.removeItem(at: url)
        }
        failed = false
        _ = openIfNeeded()
    }

    private func close() {
        try? handle?.close()
        handle = nil
        size = 0
    }

    static func timestamp(_ date: Date) -> String {
        date.formatted(Date.ISO8601FormatStyle(includingFractionalSeconds: true, timeZone: .gmt))
    }
}
