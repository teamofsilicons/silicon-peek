import AppKit
import QuartzCore

/// The per-bubble frame clock (visual.md B4): `NSView.displayLink(target:selector:)` on the canvas, which
/// follows the display's refresh rate (up to 120 Hz) and pauses while the view is off screen. The link is
/// paused whenever the drawing sleeps.
///
/// The display link stops firing while its display sleeps, or when the panel sits on no live display. A drawing
/// whose panel is ordered in must still run (a throwing or overrunning script is only detected, reported to peekd
/// and replaced by the fallback visual when its frames execute), so a watchdog timer takes over at 10 Hz whenever
/// the link has been silent for 0.25 s while running.
@MainActor
final class FrameScheduler {
    /// Called on every display refresh while running, with the frame's timestamp (CACurrentMediaTime base).
    var onTick: ((CFTimeInterval) -> Void)?
    /// Called once each time the display link stalls and the watchdog starts driving frames.
    var onStall: (() -> Void)?
    /// Whether the canvas's window is ordered in; only then does the watchdog drive frames. Set by ``attach(to:)``.
    var isVisible: () -> Bool = { false }
    /// Monotonic seconds (CACurrentMediaTime base); injectable for tests.
    var clock: () -> CFTimeInterval = { CACurrentMediaTime() }
    private(set) var isRunning = false
    /// True while the watchdog drives frames because the display link went silent.
    private(set) var isStalled = false
    private var link: CADisplayLink?
    private var target: DisplayLinkTarget?
    private var watchdog: Timer?
    private var lastTick: CFTimeInterval?

    static let stallThreshold: CFTimeInterval = 0.25
    static let watchdogInterval: CFTimeInterval = 0.1

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
        isVisible = { [weak view] in view?.window?.isVisible == true }
        lastTick = clock()
        updateWatchdog()
    }

    func detach() {
        link?.invalidate()
        link = nil
        target = nil
        isVisible = { false }
        isStalled = false
        updateWatchdog()
    }

    var isAttached: Bool { link != nil }

    func start() {
        if !isRunning { lastTick = clock() }
        isRunning = true
        link?.isPaused = false
        updateWatchdog()
    }

    func stop() {
        isRunning = false
        isStalled = false
        link?.isPaused = true
        updateWatchdog()
    }

    fileprivate func tick(_ timestamp: CFTimeInterval) {
        guard isRunning else { return }
        lastTick = clock()
        isStalled = false
        onTick?(timestamp)
    }

    /// One watchdog check: drives a frame when the link has been silent for ``stallThreshold`` while running and
    /// visible. Tests call it directly.
    func watchdogFired() {
        guard isRunning, isAttached, isVisible() else { return }
        let now = clock()
        guard now - (lastTick ?? now) >= Self.stallThreshold || isStalled else { return }
        if !isStalled {
            isStalled = true
            onStall?()
        }
        onTick?(now)
    }

    private func updateWatchdog() {
        let wanted = isRunning && isAttached
        if wanted, watchdog == nil {
            let timer = Timer(timeInterval: Self.watchdogInterval, repeats: true) { [weak self] _ in
                MainActor.assumeIsolated { self?.watchdogFired() }
            }
            RunLoop.main.add(timer, forMode: .common)
            watchdog = timer
        } else if !wanted, let timer = watchdog {
            timer.invalidate()
            watchdog = nil
        }
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
