import Foundation
import PeekCore
import Testing

@testable import PeekUI

/// Simulation's production wiring with the real ``PeekCoordinator`` as its presenter. Presenting is
/// left out (it would slide a panel onto the screen during `swift test`); these check the seams:
/// the slot table, `drawing.load` through the coordinator's own handler, and settings that are
/// mirrored to the Simulation link but never written to settings.json.
@Suite("Simulation with the real coordinator")
@MainActor
struct SettingsSimulationContractTests {
    private func liveDependencies(_ controls: SettingsSuiteControls, drawing: SettingsSuiteDrawingRuntime)
        -> SimulationDependencies
    {
        var dependencies = SimulationDependencies.live(for: controls)
        dependencies.drawing = drawing
        dependencies.makeSpeech = { SettingsSuiteSpeech() }
        dependencies.makeMic = { SettingsSuiteMic() }
        dependencies.makeImages = { SettingsSuiteImages() }
        dependencies.makeLiveBackdrop = { SettingsSuiteBackdrop() }
        dependencies.makeInputHub = { _, _, _, backdrop in SettingsSuiteHub(backdrop: backdrop) }
        dependencies.assetsDirectory = controls.paths.cachesDirectory.appendingPathComponent("Simulation-test")
        return dependencies
    }

    @Test("the live graph uses a PeekCoordinator that loads the cassette and never persists settings")
    func coordinatorAsPresenter() async throws {
        let paths = settingsSuiteTemporaryPaths()
        defer { settingsSuiteRemove(paths) }
        let controls = SettingsSuiteControls(paths: paths)
        controls.settings.hotkeyModifier = .optCmd
        let drawing = SettingsSuiteDrawingRuntime()
        let pipeline = SimulationPipeline(dependencies: liveDependencies(controls, drawing: drawing), mode: .compact)
        let coordinator = try #require(pipeline.presenter as? PeekCoordinator)
        #expect(coordinator.settings.mode == .compact, "the Simulation presenter starts in the scenario's mode")
        #expect(coordinator.settings.hotkeyModifier == .optCmd, "and otherwise inherits the live settings")

        let recorder = Task { () -> [SimulationRequestRecord] in
            var records: [SimulationRequestRecord] = []
            for await record in pipeline.link.records {
                records.append(record)
                if records.count == 1 { break }
            }
            return records
        }

        await pipeline.presenter.start()
        #expect(await pipeline.link.hasRequestHandler, "the coordinator installs its peekd→UI handler on the Simulation link")

        let assets = try SimulationAssets.materialize(into: controls.paths.cachesDirectory.appendingPathComponent("Simulation-test"))
        let key = SimulationEngine.siliconKey
        let slot = SlotState(
            index: .left, context: key.context, actorID: key.actorID, orgID: key.orgID, displayName: "Simulation", initial: "S",
            drawing: DrawingRef(sha256: assets.cassetteSHA256, path: assets.cassette.path), hotkey: false)
        pipeline.presenter.updateSlots([slot])
        #expect(coordinator.slots.contains(slot))

        let reply = await pipeline.link.request(
            .drawingLoad(
                DrawingLoadRequest(
                    context: key.context, orgID: key.orgID, actorID: key.actorID, slot: .left, scriptPath: assets.cassette.path,
                    sha256: assets.cassetteSHA256)))
        guard case .ok = reply else {
            Issue.record("the coordinator refused drawing.load: \(reply)")
            return
        }
        let host = try #require(drawing.madeHosts.first { $0.key == key })
        #expect(host.loaded?.source == SimulationCassette.data)
        #expect(pipeline.drawing.host(for: key) != nil, "the coordinator's host is the Simulation-wrapped one")

        pipeline.setMode(.normal)
        let records = await recorder.value
        #expect(records.first?.op == SettingsChangedRequest.op)
        #expect(records.first?.fields["value"] == .string("normal"))
        #expect(!FileManager.default.fileExists(atPath: paths.settingsFile.path), "Simulation never writes settings.json")

        coordinator.dismissAll()
        await pipeline.presenter.stop()
        await pipeline.link.stop()
    }
}
