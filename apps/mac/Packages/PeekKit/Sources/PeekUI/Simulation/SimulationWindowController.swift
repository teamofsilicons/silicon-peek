import AppKit
import SwiftUI

/// Owns the Simulation window. The App target's scenes are fixed (MenuBarExtra + Settings), so the
/// window is a plain `NSWindow` hosting ``SimulationView``. Closing it slides simulated bubbles out
/// and tears the Simulation presenter down.
@MainActor
public final class SimulationWindowController: NSObject, NSWindowDelegate {
    private static var current: SimulationWindowController?

    private let window: NSWindow
    private let engine: SimulationEngine

    /// Shows (or brings forward) the Simulation window for `coordinator`'s engine.
    public static func show(for coordinator: PeekCoordinator) {
        let engine = SimulationEngine.shared(for: coordinator)
        if current?.engine !== engine { current = SimulationWindowController(engine: engine) }
        // An accessory app opens windows behind the frontmost app unless it activates first (§8.11).
        NSApp.activate()
        current?.window.makeKeyAndOrderFront(nil)
    }

    private init(engine: SimulationEngine) {
        self.engine = engine
        let hosting = NSHostingController(rootView: SimulationView(engine: engine))
        hosting.sizingOptions = [.minSize]
        window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 920, height: 660),
            styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
        window.title = "Peek Simulation"
        window.contentViewController = hosting
        window.isReleasedWhenClosed = false
        window.tabbingMode = .disallowed
        window.setContentSize(NSSize(width: 920, height: 660))
        window.center()
        window.setFrameAutosaveName("ai.tos.peek.simulation")
        super.init()
        window.delegate = self
    }

    public func windowWillClose(_ notification: Notification) {
        let engine = self.engine
        Task { await engine.shutdown() }
    }
}
