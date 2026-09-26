import Foundation
import Observation
import PeekCore
import PeekDrawing
import PeekIPC

/// Settings › Diagnostics: versions, the peekd socket, and the tail of peekd.log.
@MainActor
@Observable
public final class DiagnosticsModel {
    public enum LogState: Equatable, Sendable {
        case notLoaded
        case loading
        case loaded(LogTailReader.Tail)
        case failed(LogTailReader.Failure)
    }

    public let paths: PeekPaths
    public let appInfo: SettingsAppInfo
    public let socketPath: String
    public private(set) var log: LogState = .notLoaded
    public private(set) var loadedAt: Date?

    public init(paths: PeekPaths, appInfo: SettingsAppInfo = SettingsAppInfo(bundle: .main),
                socketPath: String = DaemonSocket.resolvePath()) {
        self.paths = paths
        self.appInfo = appInfo
        self.socketPath = socketPath
    }

    public var peekdLogURL: URL { paths.peekdLog }

    /// Re-reads the end of peekd.log off the main thread.
    public func refreshLog() async {
        log = .loading
        let url = paths.peekdLog
        let result = await Task.detached(priority: .utility) { () -> Result<LogTailReader.Tail, LogTailReader.Failure> in
            do throws(LogTailReader.Failure) {
                return .success(try LogTailReader.read(url))
            } catch {
                return .failure(error)
            }
        }.value
        switch result {
        case .success(let tail): log = .loaded(tail)
        case .failure(let failure): log = .failed(failure)
        }
        loadedAt = Date()
    }

    /// A plain-text summary to paste into `peek report` or a bug report.
    public func report(controls: any PeekControlling, processInfo: ProcessInfo = .processInfo) -> String {
        var lines: [String] = []
        lines.append("Peek.app \(appInfo.versionText)" + (appInfo.bundleIdentifier.map { " [\($0)]" } ?? ""))
        lines.append("macOS \(processInfo.operatingSystemVersionString)")
        lines.append("QuickJS \(QuickJSInfo.version); glass \(controls.drawing.glassMode.rawValue)")
        lines.append("IPC protocols \(IPCProtocol.supportedProtocols.map(String.init).joined(separator: ","))")
        lines.append("peekd: \(controls.linkState.statusSentence)")
        lines.append("socket: \(socketPath)")
        lines.append("settings: \(paths.settingsFile.path)")
        let settings = controls.settings
        lines.append(
            "mode \(settings.mode.rawValue), hotkeys \(settings.hotkeyModifier.rawValue), display \(settings.display.rawValue), "
                + "backdrop \(settings.backdrop.rawValue), telemetry \(settings.telemetry), show_test_peeks \(settings.showTestPeeks), "
                + "stt_language \(settings.sttLanguage)")
        for warning in controls.settingsWarnings { lines.append("settings warning: \(warning)") }
        lines.append("microphone: \(controls.mic.permission.diagnosticsText)")
        lines.append("slots: \(controls.slots.count) registered")
        for slot in controls.slots {
            lines.append("  \(slot.index.rawValue) \(slot.context.rawValue) \(slot.actorID)[\(slot.orgID)] drawing=\(slot.drawing?.sha256.prefix(12) ?? "none")")
        }
        if let problem = controls.lastProblem { lines.append("last problem: \(problem)") }
        lines.append("peekd log: \(paths.peekdLog.path)")
        if case .loaded(let tail) = log {
            lines.append("--- last \(min(tail.lines.count, 40)) lines of peekd.log ---")
            lines.append(contentsOf: tail.lines.suffix(40))
        }
        return lines.joined(separator: "\n")
    }
}
