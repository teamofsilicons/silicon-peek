import AppKit
import PeekCore
import QuartzCore

/// Puts rendered frames on screen for one bubble (visual.md B5 "diff" and "render", D14):
///
/// * layers become AppKit sibling views inside the canvas, in op order; a run of consecutive glass fills with
///   the same transform shares one hosting view (and merges, like system glass);
/// * each position is diffed with the previous frame: same kind and hash → untouched; same kind, new hash →
///   updated in place; a different kind → that view is replaced;
/// * glass outline/option changes (and blur masks) are applied at most every 100 ms per bubble, last write
///   wins; glass transforms are applied every frame;
/// * ``isOverContent(unitPoint:)`` answers B9's hit test.
@MainActor
final class Compositor {
    enum Slot {
        case draw(DrawLayerView)
        case vibrant(VibrantLayerView)
        case glass(GlassLayerView)
        case blur(BlurLayerView)

        var view: NSView {
            switch self {
            case .draw(let v): v
            case .vibrant(let v): v
            case .glass(let v): v
            case .blur(let v): v
            }
        }
    }

    private enum Unit {
        case draw(Hash64, CGImage, vibrant: Bool)
        case glass([GlassLayer])
        case blur(BlurLayer)
    }

    let canvas: DrawingCanvasView
    private(set) var slots: [Slot] = []
    private var fallback: FallbackVisualView?
    var glassMode: GlassMode

    /// Monotonic seconds (CACurrentMediaTime); injectable for tests.
    var clock: () -> CFTimeInterval = { CACurrentMediaTime() }
    /// Schedules a deferred flush of rate-limited glass changes; injectable for tests.
    var scheduleFlush: (TimeInterval, @escaping @MainActor () -> Void) -> Void = { delay, work in
        Timer.scheduledTimer(withTimeInterval: delay, repeats: false) { _ in MainActor.assumeIsolated { work() } }
    }
    private var lastShapeApply: CFTimeInterval = -.infinity
    private var flushScheduled = false
    /// Glass/blur rebuilds actually applied (for tests and diagnostics).
    private(set) var shapeRebuilds = 0

    init(canvas: DrawingCanvasView, glassMode: GlassMode) {
        self.canvas = canvas
        self.glassMode = glassMode
        canvas.compositor = self
    }

    var isShowingFallback: Bool { fallback != nil }

    // MARK: Frames

    func apply(_ frame: RenderedFrame) {
        let units = Self.units(from: frame.layers)
        var next: [Slot] = []
        var needsShapeFlush = false
        for (index, unit) in units.enumerated() {
            let existing = index < slots.count ? slots[index] : nil
            switch (unit, existing) {
            case (.draw(let hash, let image, false), .draw(let view)?):
                view.update(hash: hash, image: image)
                next.append(.draw(view))
            case (.draw(let hash, let image, true), .vibrant(let view)?):
                view.update(hash: hash, image: image)
                next.append(.vibrant(view))
            case (.draw(let hash, let image, let vibrant), _):
                if vibrant {
                    let view = VibrantLayerView(frame: canvas.bounds)
                    view.update(hash: hash, image: image)
                    next.append(.vibrant(view))
                } else {
                    let view = DrawLayerView(frame: canvas.bounds)
                    view.update(hash: hash, image: image)
                    next.append(.draw(view))
                }
            case (.glass(let members), .glass(let view)?) where view.frosted == (glassMode == .frosted):
                if view.setMembers(members) { needsShapeFlush = true }
                next.append(.glass(view))
            case (.glass(let members), _):
                let view = GlassLayerView(frame: canvas.bounds, frosted: glassMode == .frosted)
                _ = view.setMembers(members)
                view.applyPendingShape()
                shapeRebuilds += 1
                lastShapeApply = clock()
                next.append(.glass(view))
            case (.blur(let spec), .blur(let view)?):
                if view.setSpec(spec) { needsShapeFlush = true }
                next.append(.blur(view))
            case (.blur(let spec), _):
                let view = BlurLayerView(frame: canvas.bounds, material: spec.material)
                _ = view.setSpec(spec)
                view.applyPendingShape()
                shapeRebuilds += 1
                lastShapeApply = clock()
                next.append(.blur(view))
            }
        }
        install(next)
        if needsShapeFlush { flushShapes() }
    }

    /// Groups consecutive glass layers that share a transform into one unit.
    private static func units(from layers: [RenderedLayer]) -> [Unit] {
        var units: [Unit] = []
        for layer in layers {
            switch layer {
            case .draw(let hash, let image, let vibrant):
                units.append(.draw(hash, image, vibrant: vibrant))
            case .glass(let glass):
                if case .glass(var members)? = units.last, members[0].transform == glass.transform {
                    members.append(glass)
                    units[units.count - 1] = .glass(members)
                } else {
                    units.append(.glass([glass]))
                }
            case .blur(let blur):
                units.append(.blur(blur))
            }
        }
        return units
    }

    /// Makes the canvas's subviews exactly `next` (then the fallback, if any), in order. Kept views are
    /// reordered, not re-added, and nothing is touched when the order already matches.
    private func install(_ next: [Slot]) {
        let desired = next.map(\.view) + (fallback.map { [$0] } ?? [])
        for view in desired where view.superview !== canvas {
            view.frame = canvas.bounds
        }
        let current = canvas.subviews
        if current.count != desired.count || zip(current, desired).contains(where: { $0 !== $1 }) {
            canvas.subviews = desired
        }
        slots = next
    }

    /// Applies pending glass/blur shapes now if 100 ms have passed since the last rebuild, else later.
    private func flushShapes() {
        let now = clock()
        let wait = lastShapeApply + DrawingLimits.glassUpdateInterval - now
        if wait <= 0 {
            applyPendingShapes(at: now)
        } else if !flushScheduled {
            flushScheduled = true
            scheduleFlush(wait) { [weak self] in
                guard let self else { return }
                self.flushScheduled = false
                self.flushShapes()
            }
        }
    }

    private func applyPendingShapes(at now: CFTimeInterval) {
        var applied = false
        for slot in slots {
            switch slot {
            case .glass(let view) where view.hasPendingShape:
                view.applyPendingShape()
                applied = true
                shapeRebuilds += 1
            case .blur(let view) where view.hasPendingShape:
                view.applyPendingShape()
                applied = true
                shapeRebuilds += 1
            default:
                break
            }
        }
        if applied { lastShapeApply = now }
    }

    /// Tests: runs a deferred flush immediately.
    func flushPendingShapesForTesting() { applyPendingShapes(at: clock()) }

    var hasPendingShapes: Bool {
        slots.contains { slot in
            switch slot {
            case .glass(let view): view.hasPendingShape
            case .blur(let view): view.hasPendingShape
            default: false
            }
        }
    }

    func clear() {
        for slot in slots { slot.view.removeFromSuperview() }
        slots = []
    }

    func canvasDidLayout() {
        for slot in slots where slot.view.frame != canvas.bounds {
            slot.view.frame = canvas.bounds
        }
        fallback?.frame = canvas.bounds
    }

    // MARK: Fallback

    func showFallback(initial: String) {
        clear()
        if fallback == nil {
            let view = FallbackVisualView(initial: initial)
            view.frame = canvas.bounds
            canvas.addSubview(view)
            fallback = view
        }
    }

    func hideFallback() {
        fallback?.removeFromSuperview()
        fallback = nil
    }

    // MARK: Hit testing (visual.md B9)

    /// Inside a glass/blur path, or over a drawn pixel with alpha > 0.05.
    func isOverContent(unitPoint point: CGPoint) -> Bool {
        if fallback != nil { return FallbackHitTest.contains(point) }
        for slot in slots {
            switch slot {
            case .glass(let view): if view.contains(unitPoint: point) { return true }
            case .blur(let view): if view.layerSpec?.contains(unitPoint: point) == true { return true }
            case .draw(let view): if view.alpha(atUnit: point) > 0.05 { return true }
            case .vibrant(let view): if view.alpha(atUnit: point) > 0.05 { return true }
            }
        }
        return false
    }

    /// The hosting view of the topmost interactive glass under a canvas point, so it can receive events.
    func interactiveGlassView(at canvasPoint: NSPoint) -> NSView? {
        guard canvas.bounds.width > 0 else { return nil }
        let k = 100 / canvas.bounds.width
        let unit = CGPoint(x: canvasPoint.x * k, y: canvasPoint.y * k)
        for slot in slots.reversed() {
            if case .glass(let view) = slot, view.isInteractive, view.contains(unitPoint: unit) {
                return view.hostingView
            }
        }
        return nil
    }
}

enum FallbackHitTest {
    /// The fallback glass circle covers 84% of the square.
    static func contains(_ point: CGPoint) -> Bool {
        let dx = point.x - 50, dy = point.y - 50
        return dx * dx + dy * dy <= 42 * 42
    }
}
