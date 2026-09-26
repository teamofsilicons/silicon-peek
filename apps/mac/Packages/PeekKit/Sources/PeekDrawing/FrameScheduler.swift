import AppKit
import QuartzCore

/// The per-bubble frame clock (visual.md B4): `NSView.displayLink(target:selector:)` on the canvas, which
/// follows the display's refresh rate (up to 120 Hz) and pauses while the view is off screen. The link is
/// paused whenever the drawing sleeps.
@MainActor
final class FrameScheduler {
    /// Called on every display refresh while running, with the frame's timestamp (CACurrentMediaTime base).
    var onTick: ((CFTimeInterval) -> Void)?
    private(set) var isRunning = false
    private var link: CADisplayLink?
    private var target: DisplayLinkTarget?

    /// Creates the display link for `view` (replacing any previous one).
    func attach(to view: NSView) {
        detach()
        let target = DisplayLinkTarget()
        target.scheduler = self
        let link = view.displayLink(target: target, selector: #selector(DisplayLinkTarget.tick(_:)))
        link.preferredFrameRateRange = CAFrameRateRange(minimum: 60, maximum: 120, preferred: 120)
        link.isPaused = !isRunning
        link.add(to: .main, forMode: .common)
        self.link = link
        self.target = target
    }

    func detach() {
        link?.invalidate()
        link = nil
        target = nil
    }

    var isAttached: Bool { link != nil }

    func start() {
        isRunning = true
        link?.isPaused = false
    }

    func stop() {
        isRunning = false
        link?.isPaused = true
    }

    fileprivate func tick(_ timestamp: CFTimeInterval) {
        guard isRunning else { return }
        onTick?(timestamp)
    }
}

/// The display link retains its target, so it points back at the scheduler weakly.
@MainActor
private final class DisplayLinkTarget: NSObject {
    weak var scheduler: FrameScheduler?

    @objc func tick(_ link: CADisplayLink) {
        scheduler?.tick(link.timestamp)
    }
}
