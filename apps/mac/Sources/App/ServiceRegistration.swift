import AppKit
import Darwin
import OSLog
import PeekCore
import PeekIPC
import ServiceManagement

/// Registers Peek.app as a login item and peekd as its launchd agent (BLUEPRINT §1.8, D3):
///
/// * `SMAppService.mainApp.register()` on every launch.
/// * `SMAppService.agent(plistName: "ai.tos.peek.daemon.plist")` for
///   `Contents/Library/LaunchAgents/ai.tos.peek.daemon.plist` (BundleProgram
///   `Contents/Helpers/peekd`). After a self-update the agent is unregistered and
///   registered again so launchd picks up the new bundle.
/// * When the agent needs the user's approval (or cannot be registered), the app
///   spawns `Contents/Helpers/peekd run --parent-ui` itself (new session, stdio to
///   peekd.log) and shows a one-time notice pointing at Login Items.
///
/// `PEEK_NO_SERVICES=1` (isolated runs; `PEEK_SKIP_SERVICE_REGISTRATION=1` is the older alias) skips all of
/// this: no SMAppService call of any kind and no peekd spawn. The app then only connects to the peekd
/// listening on `PEEK_DAEMON_SOCKET` (docs/development.md "Isolated run mode").
@MainActor
final class ServiceRegistration {
    static let agentPlistName = "ai.tos.peek.daemon.plist"
    static let approvalNoticeKey = "ai.tos.peek.didShowBackgroundApprovalNotice"

    struct Report {
        var mainApp: SMAppService.Status?
        var agent: SMAppService.Status?
        var spawnedFallback = false
        var problems: [String] = []
    }

    private let logger = PeekLogger(category: "services")
    private let bundle: Bundle
    private let paths: PeekPaths
    private var fallbackPID: pid_t?

    private let environment: PeekRuntimeEnvironment

    init(bundle: Bundle = .main, paths: PeekPaths = .current, environment: PeekRuntimeEnvironment = .current) {
        self.bundle = bundle
        self.paths = paths
        self.environment = environment
    }

    /// False under `PEEK_NO_SERVICES=1`: then nothing here touches SMAppService or spawns peekd.
    var servicesEnabled: Bool { !environment.noServices }

    func registerAll(afterUpdate: Bool) async -> Report {
        var report = Report()
        guard servicesEnabled else {
            logger.notice("PEEK_NO_SERVICES=1: not touching the login item or the peekd agent, and not starting peekd")
            return report
        }

        let main = SMAppService.mainApp
        // .requiresApproval means it is registered and waiting for the user; registering again only fails.
        if main.status == .notRegistered {
            do {
                try main.register()
            } catch {
                report.problems.append("login item registration failed: \(Self.describe(error)); enable Peek in System Settings › General › Login Items")
            }
        }
        report.mainApp = main.status

        let agent = SMAppService.agent(plistName: Self.agentPlistName)
        if afterUpdate {
            do {
                try await agent.unregister()
            } catch {
                logger.notice("unregistering the previous peekd agent after the update failed: \(Self.describe(error))")
            }
        }
        if afterUpdate || agent.status == .notRegistered {
            do {
                try agent.register()
            } catch {
                report.problems.append("peekd agent registration failed: \(Self.describe(error))")
            }
        }
        report.agent = agent.status

        switch agent.status {
        case .enabled:
            logger.info("peekd agent enabled")
        case .requiresApproval:
            report.spawnedFallback = spawnFallbackDaemon(into: &report)
            showApprovalNoticeOnce()
        case .notFound:
            report.problems.append("the agent plist Contents/Library/LaunchAgents/\(Self.agentPlistName) is missing from this build")
            report.spawnedFallback = spawnFallbackDaemon(into: &report)
        case .notRegistered:
            report.spawnedFallback = spawnFallbackDaemon(into: &report)
        @unknown default:
            report.problems.append("the peekd agent has an unknown status \(agent.status.rawValue)")
            report.spawnedFallback = spawnFallbackDaemon(into: &report)
        }
        return report
    }

    /// `peek app uninstall` (the `--uninstall` launch argument or peekd's `app.uninstall` relay): unregisters the
    /// login item and the peekd agent, then moves this bundle to the Trash with `NSWorkspace.recycle`.
    /// Under `PEEK_NO_SERVICES=1` the services are left alone (they were never registered by this run).
    func uninstall() async -> UninstallReport {
        var report = UninstallReport(bundlePath: bundle.bundleURL.path)
        if servicesEnabled {
            for (name, service) in [("peekd agent", SMAppService.agent(plistName: Self.agentPlistName)),
                                    ("login item", SMAppService.mainApp)] {
                guard service.status != .notRegistered, service.status != .notFound else { continue }
                do {
                    try await service.unregister()
                    report.unregistered.append(name)
                } catch {
                    report.problems.append("unregistering the \(name) failed: \(Self.describe(error))")
                }
            }
        } else {
            logger.notice("PEEK_NO_SERVICES=1: uninstall leaves the login item and the peekd agent alone")
        }
        if let pid = fallbackPID, kill(pid, 0) == 0 { kill(pid, SIGTERM) }
        do {
            let recycled = try await NSWorkspace.shared.recycle([bundle.bundleURL])
            report.trashedPath = recycled[bundle.bundleURL]?.path
        } catch {
            report.problems.append("moving \(bundle.bundleURL.path) to the Trash failed: \(Self.describe(error))")
        }
        for problem in report.problems { logger.error("uninstall: \(problem)") }
        logger.notice("uninstall: unregistered [\(report.unregistered.joined(separator: ", "))], trash: \(report.trashedPath ?? "not moved")")
        return report
    }

    struct UninstallReport: Sendable {
        var bundlePath: String
        var unregistered: [String] = []
        var trashedPath: String?
        var problems: [String] = []
    }

    /// Starts `Contents/Helpers/peekd run --parent-ui` in its own session with stdio appended to peekd.log.
    /// peekd's own single-instance lock makes a duplicate exit at once with `daemon_running`.
    private func spawnFallbackDaemon(into report: inout Report) -> Bool {
        if let pid = fallbackPID, kill(pid, 0) == 0 { return true }
        let helper = bundle.bundleURL.appendingPathComponent("Contents/Helpers/peekd")
        guard FileManager.default.isExecutableFile(atPath: helper.path) else {
            report.problems.append("\(helper.path) is missing; this build embeds no peekd (see scripts/embed-peekd.sh)")
            return false
        }
        do {
            try FileManager.default.createDirectory(
                at: paths.supportDirectory, withIntermediateDirectories: true,
                attributes: [.posixPermissions: NSNumber(value: 0o700)])
        } catch {
            report.problems.append("cannot create \(paths.supportDirectory.path): \(error.localizedDescription)")
            return false
        }

        var actions: posix_spawn_file_actions_t?
        posix_spawn_file_actions_init(&actions)
        defer { posix_spawn_file_actions_destroy(&actions) }
        posix_spawn_file_actions_addopen(&actions, 0, "/dev/null", O_RDONLY, 0)
        posix_spawn_file_actions_addopen(&actions, 1, paths.peekdLog.path, O_WRONLY | O_CREAT | O_APPEND, 0o600)
        posix_spawn_file_actions_adddup2(&actions, 1, 2)

        var attributes: posix_spawnattr_t?
        posix_spawnattr_init(&attributes)
        defer { posix_spawnattr_destroy(&attributes) }
        posix_spawnattr_setflags(&attributes, Int16(POSIX_SPAWN_SETSID | POSIX_SPAWN_CLOEXEC_DEFAULT))

        let arguments = [helper.path, "run", "--parent-ui"]
        var argv: [UnsafeMutablePointer<CChar>?] = arguments.map { strdup($0) } + [nil]
        defer { for pointer in argv { free(pointer) } }
        var pid: pid_t = 0
        let status = posix_spawn(&pid, helper.path, &actions, &attributes, &argv, environ)
        guard status == 0 else {
            report.problems.append("spawning \(helper.path) failed: \(String(cString: strerror(status)))")
            return false
        }
        fallbackPID = pid
        logger.notice("started peekd directly (pid \(pid)) because the launchd agent is not enabled")
        return true
    }

    private func showApprovalNoticeOnce() {
        let defaults = UserDefaults.standard
        guard !defaults.bool(forKey: Self.approvalNoticeKey) else { return }
        defaults.set(true, forKey: Self.approvalNoticeKey)
        NSApp.activate()
        let alert = NSAlert()
        alert.messageText = "Allow Peek in the background"
        alert.informativeText = "Peek runs a small helper (peekd) that delivers your Silicons' peeks and answers. "
            + "macOS needs your approval before it can start on its own: turn Peek on in System Settings › General › Login Items. "
            + "Until then Peek starts the helper itself while the app is open."
        alert.addButton(withTitle: "Open Login Items")
        alert.addButton(withTitle: "Later")
        if alert.runModal() == .alertFirstButtonReturn {
            SMAppService.openSystemSettingsLoginItems()
        }
    }

    private static func describe(_ error: any Error) -> String {
        let nsError = error as NSError
        return "\(nsError.localizedDescription) (\(nsError.domain) \(nsError.code))"
    }
}
