import Foundation
import PeekCore

/// What the app does when peekd relaunched it after swapping in a new bundle
/// (`--after-update <old build>`, Silicon Apps install hook). The agent re-registration
/// itself happens in ``ServiceRegistration/registerAll(afterUpdate:)``; this type
/// reads peekd's `update-applied.json` and reports the update.
struct SelfUpdateHooks {
    /// `update-applied.json` written by peekd right before the relaunch.
    struct Applied: Decodable, Equatable {
        var from: Int?
        var to: Int?
        var at: JSONValue?
    }

    let arguments: AppLaunchArguments
    let paths: PeekPaths
    let bundle: Bundle

    init(arguments: AppLaunchArguments, paths: PeekPaths, bundle: Bundle = .main) {
        self.arguments = arguments
        self.paths = paths
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

    /// peekd's record of the swap, when present and readable.
    func appliedRecord() -> Applied? {
        guard let data = try? Data(contentsOf: paths.updateAppliedFile) else { return nil }
        return try? JSONDecoder().decode(Applied.self, from: data)
    }

    /// The `app.updated` telemetry event for this launch, or nil when this is not a post-update launch.
    func updateTelemetry() -> TelemetryEvent? {
        guard let from = arguments.afterUpdateFromBuild else { return nil }
        var data: [String: JSONValue] = ["from_build": .int(Int64(from)), "to_build": .int(Int64(currentBuild))]
        if let applied = appliedRecord() {
            if let recordedTo = applied.to { data["recorded_to_build"] = .int(Int64(recordedTo)) }
            if let at = applied.at { data["applied_at"] = at }
        }
        return TelemetryEvent(type: "app.updated", data: .object(data), metadata: ["source": "peek.app"])
    }
}
