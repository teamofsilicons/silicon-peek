import AppKit
import CoreGraphics
import PeekCore

/// What decides whether the Carbon can see bubbles. Pure, so the mapping is testable without a lock screen.
public struct PresenceState: Sendable, Equatable {
    /// `com.apple.screenIsLocked` (or `CGSSessionScreenIsLocked` at launch).
    public var screenLocked = false
    /// This login session is not on the console (fast user switching: `sessionDidResignActive`).
    public var sessionInactive = false
    /// `NSWorkspace.screensDidSleep` (or `CGDisplayIsAsleep` at launch).
    public var displaysAsleep = false
    /// No screen at all (closed lid without an external display).
    public var noDisplays = false

    public init(screenLocked: Bool = false, sessionInactive: Bool = false, displaysAsleep: Bool = false,
                noDisplays: Bool = false) {
        self.screenLocked = screenLocked
        self.sessionInactive = sessionInactive
        self.displaysAsleep = displaysAsleep
        self.noDisplays = noDisplays
    }

    /// The `presence` request for this state. A lock wins over sleep: a locked Mac whose display wakes is still away.
    public var request: PresenceRequest {
        if screenLocked || sessionInactive { return PresenceRequest(available: false, reason: .locked) }
        if displaysAsleep { return PresenceRequest(available: false, reason: .asleep) }
        if noDisplays { return PresenceRequest(available: false, reason: .displayOff) }
        return .available
    }
}

/// Watches the lock screen, display sleep, session switches and screen changes, and reports every change of the
/// resulting ``PresenceRequest`` (the first report is the state at ``start()``). Only the real app runs it.
@MainActor
public final class PresenceMonitor {
    public private(set) var state: PresenceState
    private var lastReported: PresenceRequest?
    private let onChange: @MainActor (PresenceRequest) -> Void
    private var workspaceObservers: [any NSObjectProtocol] = []
    private var distributedObservers: [any NSObjectProtocol] = []
    private var localObservers: [any NSObjectProtocol] = []

    public init(onChange: @escaping @MainActor (PresenceRequest) -> Void) {
        self.onChange = onChange
        state = PresenceState()
    }

    isolated deinit { stop() }

    public func start() {
        guard workspaceObservers.isEmpty else { return }
        state = Self.currentState()
        let workspace = NSWorkspace.shared.notificationCenter
        func onWorkspace(_ name: Notification.Name, _ change: @escaping @MainActor (inout PresenceState) -> Void) {
            workspaceObservers.append(workspace.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated { self?.update(change) }
            })
        }
        onWorkspace(NSWorkspace.screensDidSleepNotification) { $0.displaysAsleep = true }
        onWorkspace(NSWorkspace.screensDidWakeNotification) { $0.displaysAsleep = false }
        onWorkspace(NSWorkspace.sessionDidResignActiveNotification) { $0.sessionInactive = true }
        onWorkspace(NSWorkspace.sessionDidBecomeActiveNotification) { state in
            state.sessionInactive = false
            state.screenLocked = Self.sessionScreenIsLocked()
        }

        let distributed = DistributedNotificationCenter.default()
        for (name, locked) in [("com.apple.screenIsLocked", true), ("com.apple.screenIsUnlocked", false)] {
            distributedObservers.append(distributed.addObserver(
                forName: Notification.Name(name), object: nil, queue: .main
            ) { [weak self] _ in
                MainActor.assumeIsolated { self?.update { $0.screenLocked = locked } }
            })
        }
        localObservers.append(NotificationCenter.default.addObserver(
            forName: NSApplication.didChangeScreenParametersNotification, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated { self?.update { $0.noDisplays = NSScreen.screens.isEmpty } }
        })
        report()
    }

    public func stop() {
        for observer in workspaceObservers { NSWorkspace.shared.notificationCenter.removeObserver(observer) }
        for observer in distributedObservers { DistributedNotificationCenter.default().removeObserver(observer) }
        for observer in localObservers { NotificationCenter.default.removeObserver(observer) }
        workspaceObservers.removeAll()
        distributedObservers.removeAll()
        localObservers.removeAll()
    }

    /// Applies one change and reports the result if it differs from the last report (tests call it directly).
    func update(_ change: (inout PresenceState) -> Void) {
        change(&state)
        report()
    }

    private func report() {
        let request = state.request
        guard request != lastReported else { return }
        lastReported = request
        onChange(request)
    }

    /// The state right now, for launch (the app may start while the screen is locked or asleep).
    static func currentState() -> PresenceState {
        let session = CGSessionCopyCurrentDictionary() as? [String: Any]
        let onConsole = (session?[kCGSessionOnConsoleKey as String] as? Bool) ?? true
        return PresenceState(
            screenLocked: (session?["CGSSessionScreenIsLocked"] as? Bool) ?? false,
            sessionInactive: !onConsole,
            displaysAsleep: CGDisplayIsAsleep(CGMainDisplayID()) != 0,
            noDisplays: NSScreen.screens.isEmpty)
    }

    static func sessionScreenIsLocked() -> Bool {
        let session = CGSessionCopyCurrentDictionary() as? [String: Any]
        return (session?["CGSSessionScreenIsLocked"] as? Bool) ?? false
    }
}
