import AppKit
import Foundation
import PeekCore

/// Converts the pointer into a drawing's 100 × 100 units (visual.md A4 `mouse`, B8 `hover, mouse`).
///
/// Screen points are AppKit global coordinates (`NSEvent.mouseLocation`, y up). Units are y-down from the visual
/// square's top-left corner and may lie outside 0…100. The visual circle is inscribed in the square (r = 50).
public enum PointerMath {
    /// `point` in units of the visual square `visualFrameOnScreen`; nil when the square is empty.
    public static func units(fromScreen point: CGPoint, visualFrameOnScreen frame: CGRect) -> CGPoint? {
        guard frame.width > 0, frame.height > 0, point.x.isFinite, point.y.isFinite else { return nil }
        return CGPoint(x: (point.x - frame.minX) * 100 / frame.width, y: (frame.maxY - point.y) * 100 / frame.height)
    }

    /// `input.mouse` for a pointer at `point`: position, distance and angle from (50, 50), and `inside`.
    /// ``InputSnapshot/Mouse/outside`` when the square is empty.
    public static func mouse(fromScreen point: CGPoint, visualFrameOnScreen frame: CGRect) -> InputSnapshot.Mouse {
        guard let units = units(fromScreen: point, visualFrameOnScreen: frame) else { return .outside }
        return InputSnapshot.Mouse(x: Double(units.x), y: Double(units.y))
    }

    /// `input.mouse` for a slot layout (the same square as ``SlotLayout/visualFrameOnScreen``).
    public static func mouse(fromScreen point: CGPoint, layout: SlotLayout) -> InputSnapshot.Mouse {
        mouse(fromScreen: point, visualFrameOnScreen: layout.visualFrameOnScreen)
    }

    /// The current pointer (`NSEvent.mouseLocation`) in a slot's units.
    @MainActor
    public static func currentMouse(layout: SlotLayout) -> InputSnapshot.Mouse {
        mouse(fromScreen: NSEvent.mouseLocation, layout: layout)
    }
}

/// Where the pointer is, and a callback when it moves. Injected into ``InputHub`` for tests.
@MainActor
public protocol PointerTracking: AnyObject {
    /// The pointer in AppKit global screen coordinates.
    var location: CGPoint { get }
    /// Called on every pointer move while monitoring.
    var onMove: (@MainActor () -> Void)? { get set }
    var isMonitoring: Bool { get }
    func startMonitoring()
    func stopMonitoring()
}

/// The live pointer: `NSEvent.mouseLocation`, plus a global monitor (other apps active; needs no permission for
/// mouse events) and a local monitor (Peek's own panels or Settings key) for moves and drags (§8.5, B8).
@MainActor
public final class SystemPointer: PointerTracking {
    public var onMove: (@MainActor () -> Void)?
    private var globalMonitor: Any?
    private var localMonitor: Any?

    public init() {}

    public var location: CGPoint { NSEvent.mouseLocation }
    public var isMonitoring: Bool { globalMonitor != nil || localMonitor != nil }

    public func startMonitoring() {
        guard !isMonitoring else { return }
        let mask: NSEvent.EventTypeMask = [.mouseMoved, .leftMouseDragged, .rightMouseDragged, .otherMouseDragged]
        let notify = MainThread.callback { [weak self] in self?.onMove?() }
        globalMonitor = NSEvent.addGlobalMonitorForEvents(matching: mask) { _ in notify() }
        localMonitor = NSEvent.addLocalMonitorForEvents(matching: mask) { event in
            notify()
            return event
        }
    }

    public func stopMonitoring() {
        if let globalMonitor { NSEvent.removeMonitor(globalMonitor) }
        if let localMonitor { NSEvent.removeMonitor(localMonitor) }
        globalMonitor = nil
        localMonitor = nil
    }
}

/// Runs main-actor code from callbacks that AppKit, Foundation or AVFoundation may invoke on other threads.
/// The closures are built outside any actor: a closure written inside a `@MainActor` method is main-actor
/// isolated in Swift 6 and traps when called elsewhere.
enum MainThread {
    static func callback(_ body: @escaping @MainActor @Sendable () -> Void) -> @Sendable () -> Void {
        { run(body) }
    }

    static func run(_ body: @escaping @MainActor @Sendable () -> Void) {
        if Thread.isMainThread {
            MainActor.assumeIsolated { body() }
        } else {
            DispatchQueue.main.async { MainActor.assumeIsolated { body() } }
        }
    }
}
