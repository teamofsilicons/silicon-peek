import AppKit
import PeekCore
import PeekUI

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    let launch = AppLaunchArguments.parse(CommandLine.arguments)
    let environment = PeekRuntimeEnvironment.current
    let coordinator = PeekComposition.makeLive()

    private lazy var services = ServiceRegistration(paths: coordinator.paths, environment: environment)
    private let logger = PeekLogger(category: "app")
    private var terminating = false

    func applicationWillFinishLaunching(_ notification: Notification) {
        // ui.log (10 MB × 3) in the support directory, which PEEK_SUPPORT_DIR moves for isolated runs.
        PeekLog.configure(file: coordinator.paths.uiLog)
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        // LSUIElement already makes this an accessory app; be explicit for `swift run`-style launches.
        NSApp.setActivationPolicy(.accessory)
        disableTextSubstitutions()

        let hooks = SelfUpdateHooks(arguments: launch, paths: coordinator.paths)
        hooks.logLaunch(logger: logger)
        logIsolation()
        for warning in coordinator.settingsWarnings {
            logger.warning("settings.json: \(warning)")
        }

        if launch.uninstall {
            // `peek app uninstall` fallback (`open -g -j Peek.app --args --uninstall`): no bubbles, no peekd link.
            Task { await uninstallAndQuit() }
            return
        }

        coordinator.servicesEnabled = services.servicesEnabled
        coordinator.uninstallHandler = { [weak self] in await self?.uninstallAndQuit() }

        Task {
            if launch.wantsSimulation {
                // Screenshot and Simulation launches never register a login item or the launchd agent.
                logger.notice("--simulate: not registering the login item or the peekd agent for this run")
            } else {
                let report = await services.registerAll(afterUpdate: launch.isAfterUpdate)
                for problem in report.problems {
                    logger.error("service registration: \(problem)")
                }
            }
            await coordinator.start()
            if let event = hooks.updateTelemetry() {
                do throws(DaemonLinkError) {
                    _ = try await coordinator.link.send(TelemetryRequest(events: [event]))
                } catch {
                    logger.notice("could not report the update: \(error.description)")
                }
            }
        }
        // `--simulate <scenario>`: present a Simulation bubble once the app is up (idempotent; the menu bar
        // label calls it too).
        SimulationLaunchHook.scheduleIfRequested(for: coordinator)
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard !terminating else { return .terminateNow }
        terminating = true
        coordinator.dismissAll()
        Task {
            await SimulationLaunchHook.shutdown(for: coordinator)
            await coordinator.stop()
            NSApp.reply(toApplicationShouldTerminate: true)
        }
        return .terminateLater
    }

    // MARK: Uninstall

    private var uninstalling = false

    /// Unregisters the services (unless `PEEK_NO_SERVICES=1`), moves this bundle to the Trash, then quits.
    private func uninstallAndQuit() async {
        guard !uninstalling else { return }
        uninstalling = true
        coordinator.dismissAll()
        let report = await services.uninstall()
        FileHandle.standardError.write(Data((
            "peek: uninstall unregistered [\(report.unregistered.joined(separator: ", "))]; "
                + "bundle \(report.trashedPath.map { "moved to \($0)" } ?? "not moved")"
                + (report.problems.isEmpty ? "" : "; problems: " + report.problems.joined(separator: "; ")) + "\n").utf8))
        NSApp.terminate(nil)
    }

    // MARK: Helpers

    private func logIsolation() {
        let paths = coordinator.paths
        guard paths.isIsolated || environment.noServices || environment.daemonSocket != nil || environment.apiURL != nil
        else { return }
        logger.notice(
            "isolated run: support \(paths.supportDirectory.path), caches \(paths.cachesDirectory.path), "
                + "socket \(environment.daemonSocket ?? "default"), api \(environment.apiURL ?? "default"), "
                + "services \(environment.noServices ? "off (PEEK_NO_SERVICES)" : "on")")
    }

    /// The typing field must send exactly what the Carbon typed (silicon-mac AppDelegate precedent). The keys go
    /// into the volatile argument domain, which outranks the global domain and is never written to disk.
    private func disableTextSubstitutions() {
        let defaults = UserDefaults.standard
        var domain = defaults.volatileDomain(forName: UserDefaults.argumentDomain)
        for key in ["NSAutomaticQuoteSubstitutionEnabled", "NSAutomaticDashSubstitutionEnabled",
                    "NSAutomaticTextReplacementEnabled", "NSAutomaticSpellingCorrectionEnabled",
                    "NSAutomaticPeriodSubstitutionEnabled"] {
            domain[key] = false
        }
        defaults.setVolatileDomain(domain, forName: UserDefaults.argumentDomain)
    }
}
