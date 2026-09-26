import PeekCore

/// What the menu bar, Settings and Simulation read from and change in the running app.
///
/// ``PeekCoordinator`` provides all of it (the conformance below is empty on purpose: every
/// requirement is an existing public member of the coordinator). Keeping the dependency in one
/// protocol means the ui-settings views never reach into coordinator internals, and tests drive
/// the views' models with an in-memory fake instead of a live peekd connection.
///
/// All reads go through `@Observable` properties of the conforming object, so SwiftUI views that
/// read them through `any PeekControlling` still re-render when they change.
@MainActor
public protocol PeekControlling: AnyObject {
    /// The link to peekd (connected, waiting with a reason, …).
    var linkState: DaemonLinkState { get }
    /// The slot table from peekd's latest `slots.state`, every context.
    var slots: [SlotState] { get }
    /// The settings in effect (settings.json as last applied).
    var settings: PeekSettings { get }
    /// Values from settings.json that were ignored when it was loaded.
    var settingsWarnings: [String] { get }
    /// Silences Silicon-initiated peeks; they stay queued in peekd.
    var paused: Bool { get set }
    /// The most recent problem worth showing to the Carbon (a failed save, a malformed event, …).
    var lastProblem: String? { get }
    /// Position shortcuts macOS refused (already taken by another app), one sentence each with the fix; shown under
    /// Settings › Keyboard, which those sentences point to.
    var hotkeyProblems: [String] { get }
    var paths: PeekPaths { get }
    /// The live drawing runtime (glass mode for Diagnostics; Simulation reuses it).
    var drawing: any DrawingRuntimeProviding { get }
    /// The live microphone (permission state for Diagnostics).
    var mic: any MicRecording { get }
    /// Applies one setting: persists settings.json and mirrors it to peekd with `settings.changed`.
    func setSetting(_ key: PeekSettings.Key, _ value: JSONValue)
}

extension PeekCoordinator: PeekControlling {}
