import Foundation
import PeekCore

/// What the app does when peekd relaunched it after swapping in a new bundle
/// (`--after-update <old build>`, Silicon Apps install hook). The agent re-registration
/// itself happens in ``ServiceRegistration/registerAll(afterUpdate:)``; this type
/// writes launch details to the local app log.
struct SelfUpdateHooks {
    let arguments: AppLaunchArguments
    let bundle: Bundle

    init(arguments: AppLaunchArguments, bundle: Bundle = .main) {
        self.arguments = arguments
        self.bundle = bundle
    }

    var currentBuild: Int {
        (bundle.infoDictionary?["CFBundleVersion"] as? String).flatMap(Int.init) ?? 0
    }

    func logLaunch(logger: PeekLogger) {
        if let from = arguments.afterUpdateFromBuild {
            logger.notice("relaunched after an update from build \(from) to \(currentBuild)")
        }
        if let launchedBy = arguments.launchedBy {
            logger.info("launched by \(launchedBy)")
        }
        if !arguments.simulation.isEmpty {
            logger.info("simulation flags: \(arguments.simulation.joined(separator: " "))")
        }
        if arguments.uninstall {
            logger.notice("--uninstall: unregistering the services and moving this bundle to the Trash")
        }
        if !arguments.unrecognized.isEmpty {
            logger.notice("ignoring unknown arguments: \(arguments.unrecognized.joined(separator: " "))")
        }
    }
}
