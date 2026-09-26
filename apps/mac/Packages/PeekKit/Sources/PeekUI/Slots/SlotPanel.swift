import AppKit
import OSLog
import PeekCore
import QuartzCore
import SwiftUI

/// Real Liquid Glass in an always-inactive panel (D13, notes/gap-glass-inactive-panel §0, §4).
///
/// Glass renders its key-window look only when the window reports an active appearance. AppKit
/// asks the private `-[NSWindow _hasActiveAppearance]`; ``ActiveLookSlotPanel`` answers YES. This
/// is private SPI, so it is gated: the startup self-check below uses the override only when AppKit
/// still implements that selector, and otherwise selects the public frosted look (`input.glass`).
public enum GlassSupport {
    public static let privateSelectorName = "_hasActiveAppearance"

    /// The startup self-check: `live` when the private selector exists on NSWindow, else `frosted`.
    public static func selfCheck() -> GlassMode {
        NSWindow.instancesRespond(to: NSSelectorFromString(privateSelectorName)) ? .live : .frosted
    }
}

/// The pointing-hand cursor over interactive chrome (ui-feedback.md #2) while Peek is not the active app.
///
/// PRIVATE SPI, gated like ``GlassSupport``: macOS lets only the active app set the cursor, and Peek is never active
/// (its panels are non-activating). The window-server connection property `SetsCursorInBackground` lifts that for
/// Peek's own windows. Both symbols are looked up at run time; when either is missing (or the call fails) the
/// cursor simply stays the system's, and the rim/lift/spring still mark what is clickable.
@MainActor
enum BackgroundCursor {
    static let isEnabled: Bool = {
        typealias Connection = @convention(c) () -> Int32
        typealias SetProperty = @convention(c) (Int32, Int32, CFString, CFTypeRef) -> Int32
        guard let handle = dlopen(nil, RTLD_NOW),
              let connection = dlsym(handle, "_CGSDefaultConnection"),
              let setProperty = dlsym(handle, "CGSSetConnectionProperty") else { return false }
        let cid = unsafeBitCast(connection, to: Connection.self)()
        let status = unsafeBitCast(setProperty, to: SetProperty.self)(cid, cid, "SetsCursorInBackground" as CFString,
                                                                        kCFBooleanTrue)
        return status == 0
    }()
}

/// A slot's window (visual.md B2, BLUEPRINT §8.3): borderless, non-activating, floating on every
/// Space and over full-screen apps, never animated by AppKit, never moved or resized once placed.
/// It becomes key only while the Carbon types or talks after a summon, and never activates the app.
public class SlotPanel: NSPanel {
    /// `canBecomeKey`: true only while typing or voice after a summon (§8.5).
    var allowKey = false
    /// Key presses while key; return true when handled (the event is then not dispatched).
    var keyHandler: ((NSEvent) -> Bool)?
    var resignKeyHandler: (() -> Void)?

    init(contentRect: NSRect) {
        super.init(contentRect: contentRect, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        isOpaque = false
        backgroundColor = .clear
        hasShadow = false  // shadows are drawn per element
        level = .floating
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary, .ignoresCycle]
        isMovable = false
        isMovableByWindowBackground = false
        hidesOnDeactivate = false
        isFloatingPanel = true
        becomesKeyOnlyIfNeeded = true
        animationBehavior = .none
        ignoresMouseEvents = true
        isReleasedWhenClosed = false
        allowsToolTipsWhenApplicationIsInactive = true
        titleVisibility = .hidden
        titlebarAppearsTransparent = true
    }

    public override var canBecomeKey: Bool { allowKey }
    public override var canBecomeMain: Bool { false }

    public override func sendEvent(_ event: NSEvent) {
        if event.type == .keyDown, isKeyWindow, let keyHandler, keyHandler(event) { return }
        super.sendEvent(event)
    }

    public override func resignKey() {
        super.resignKey()
        resignKeyHandler?()
    }

    /// Esc is handled by `keyHandler`; never let AppKit's cancel action close anything.
    public override func cancelOperation(_ sender: Any?) {}
}

/// PRIVATE SPI (macOS 27.0, verified in notes/gap-glass §3.2): with YES, SwiftUI `glassEffect` and
/// NSGlassEffectView render real Liquid Glass (lensing, refraction, rims) while the panel is not key
/// and the app is inactive. Only instantiated when ``GlassSupport/selfCheck()`` returns `live`.
final class ActiveLookSlotPanel: SlotPanel {
    @objc func _hasActiveAppearance() -> Bool { true }
}

/// The drawing's 100 × 100 unit square. The drawing host puts its layers here; clicks are taken by
/// the root view (so they become `click` events), never by the layers.
final class VisualSquareView: NSView {
    override var isFlipped: Bool { true }

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.masksToBounds = true
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { nil }

    override func hitTest(_ point: NSPoint) -> NSView? { nil }
}

/// The SwiftUI chrome. It accepts the first click, since the panel is never key when clicked.
final class ChromeHostingView: NSHostingView<SlotChromeView> {
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
}

/// Holds the visual and the chrome; the slide animates this view's layer position.
final class SlideContainerView: NSView {
    override var isFlipped: Bool { true }

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { nil }
}

/// The panel's content view: routes the pointer (B9) and catches down-arrow and visual clicks.
final class SlotRootView: NSView {
    let container: SlideContainerView
    let visualView: VisualSquareView
    let chromeHost: ChromeHostingView
    let chrome: SlotChromeModel
    var interaction: SlotInteraction = .none
    var visualHitTest: ((CGPoint) -> Bool)?
    var onVisualClick: ((CGPoint, Int) -> Void)?
    var onDownArrowClick: ((Int) -> Void)?

    init(frame: NSRect, chrome: SlotChromeModel) {
        self.chrome = chrome
        container = SlideContainerView(frame: NSRect(origin: .zero, size: frame.size))
        visualView = VisualSquareView(frame: chrome.slotLayout.visualRect)
        chromeHost = ChromeHostingView(rootView: SlotChromeView(model: chrome))
        super.init(frame: frame)
        wantsLayer = true
        chromeHost.sizingOptions = []
        chromeHost.frame = container.bounds
        container.addSubview(visualView)
        container.addSubview(chromeHost)
        addSubview(container)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { nil }

    override var isFlipped: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    private var layout: SlotLayout { chrome.slotLayout }

    private func overDownArrow(_ point: CGPoint) -> Bool {
        let buttons = chrome.layout.buttons
        return hypot(point.x - buttons.down.x, point.y - buttons.down.y) <= buttons.downRadius + 3
    }

    /// The open tap-to-expand popup sits above everything and takes the pointer.
    private func overPopup(_ point: CGPoint) -> Bool {
        chrome.popupOverlay?.frame.contains(point) ?? false
    }

    private func overVisual(_ point: CGPoint) -> Bool {
        let rect = layout.visualRect
        guard rect.contains(point) else { return false }
        let unit = layout.drawingUnits(fromPanel: point)
        if let visualHitTest { return visualHitTest(unit) }
        let dx = unit.x - 50, dy = unit.y - 50
        return dx * dx + dy * dy <= 2500
    }

    /// B9: over the chrome, the down-arrow, or a drawn pixel of the visual.
    func isOverContent(_ point: CGPoint) -> Bool {
        switch interaction {
        case .none: return false
        case .downArrowOnly: return overDownArrow(point)
        case .full: return overPopup(point) || overDownArrow(point) || chrome.layout.isOverChrome(point) || overVisual(point)
        }
    }

    override func hitTest(_ point: NSPoint) -> NSView? {
        let local = superview.map { convert(point, from: $0) } ?? point
        switch interaction {
        case .none:
            return nil
        case .downArrowOnly:
            return overDownArrow(local) ? self : nil
        case .full:
            if overPopup(local) {
                let inContainer = container.convert(local, from: self)
                return chromeHost.hitTest(inContainer) ?? chromeHost
            }
            if overDownArrow(local) { return self }
            if chrome.layout.isOverChrome(local) {
                let inContainer = container.convert(local, from: self)
                return chromeHost.hitTest(inContainer) ?? chromeHost
            }
            return overVisual(local) ? self : nil
        }
    }

    override func mouseDown(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        if overDownArrow(point) {
            chrome.setPressed(.down)
            onDownArrowClick?(event.clickCount)
        } else if interaction == .full, layout.visualRect.contains(point) {
            onVisualClick?(layout.drawingUnits(fromPanel: point), event.clickCount)
        }
    }

    override func mouseUp(with event: NSEvent) {
        chrome.setPressed(nil)
    }
}

/// The live ``SlotSurface``: one ``SlotPanel`` per physical slot (visual.md B2, BLUEPRINT §8.3–§8.5).
///
/// * Sized once for the slot's largest state and never moved or resized while visible.
/// * The slide is a layer animation of the content from beyond `NSScreen.visibleFrame`'s edge:
///   `CASpringAnimation(perceptualDuration: 0.5, bounce: 0.22)` in ("like it was hiding behind the
///   side, macOS-style with a little bounce"), a bounce-free 0.3 s spring out (`.smooth(duration: 0.3)`).
/// * Click-through: on every display-link tick the pointer is polled and
///   `ignoresMouseEvents = !isOverContent(pointer)` (transparent pixels alone do not pass clicks through).
@MainActor
public final class SlotPanelController: NSObject, SlotSurface {
    public let chrome: SlotChromeModel
    public var visualView: NSView { root.visualView }
    public var isKeyFocused: Bool { panel.isKeyWindow }
    public var onKey: ((KeyInput) -> Bool)?
    public var onResignKey: (() -> Void)?
    public var onVisualClick: ((CGPoint, Int) -> Void)? {
        didSet { root.onVisualClick = onVisualClick }
    }
    public var onDownArrowClick: ((Int) -> Void)? {
        didSet { root.onDownArrowClick = onDownArrowClick }
    }
    public var onTick: (() -> Void)?
    public var visualHitTest: ((CGPoint) -> Bool)? {
        didSet { root.visualHitTest = visualHitTest }
    }

    let panel: SlotPanel
    private let root: SlotRootView
    private var displayLink: CADisplayLink?
    private var sliding: SlideState = .hidden
    private var dragging = false
    private let logger = PeekLogger(category: "slots")
    /// Esc closes an open popup while the panel is not key.
    private let escapeKey = EscapeHotKey()
    /// The pointing-hand cursor is showing over interactive chrome (ui-feedback.md #2).
    private var showsPointingHand = false
    #if DEBUG
    private var debugHooksApplied = false
    #endif

    private enum SlideState { case hidden, entering, shown, leaving }

    public static let enterSpring = (perceptualDuration: 0.5, bounce: 0.22)
    public static let exitSpring = (perceptualDuration: 0.3, bounce: 0.0)

    public init(chrome: SlotChromeModel, glass: GlassMode) {
        self.chrome = chrome
        let frame = chrome.slotLayout.panelFrame
        panel = glass == .live ? ActiveLookSlotPanel(contentRect: frame) : SlotPanel(contentRect: frame)
        root = SlotRootView(frame: NSRect(origin: .zero, size: frame.size), chrome: chrome)
        super.init()
        panel.contentView = root
        panel.keyHandler = { [weak self] event in self?.handleKey(event) ?? false }
        panel.resignKeyHandler = { [weak self] in self?.onResignKey?() }
        chrome.actions.dragging = { [weak self] active in self?.dragging = active }
        chrome.onExpandedChanged = { [weak self] open in self?.escapeKey.setActive(open) }
        escapeKey.onPress = { [weak self] in self?.chrome.collapse() }
        placeContent(hidden: true)
    }

    public func configure(layout: SlotLayout) {
        if sliding == .leaving, layout.panelFrame != panel.frame {
            // A new layout (mode or screen change) while the last bubble is still sliding out: finish hiding now.
            root.container.layer?.removeAnimation(forKey: "slide")
            hideNow()
        }
        chrome.setSlotLayout(layout)
        guard sliding == .hidden else { return }
        panel.setFrame(layout.panelFrame, display: false)
        root.frame = NSRect(origin: .zero, size: layout.panelSize)
        root.container.frame = NSRect(origin: .zero, size: layout.panelSize)
        root.chromeHost.frame = root.container.bounds
        root.visualView.frame = layout.visualRect
        placeContent(hidden: true)
    }

    public func slideIn() {
        let layout = chrome.slotLayout
        let hidden = CGPoint(x: layout.hiddenOffset.dx, y: layout.hiddenOffset.dy)
        let from: CGPoint
        switch sliding {
        case .hidden:
            placeContent(hidden: true)
            from = root.container.layer?.position ?? hidden
            panel.orderFrontRegardless()
        case .leaving, .entering, .shown:
            from = root.container.layer?.presentation()?.position ?? root.container.layer?.position ?? hidden
        }
        sliding = .entering
        placeContent(hidden: false)
        let to = root.container.layer?.position ?? .zero
        let spring = CASpringAnimation(perceptualDuration: Self.enterSpring.perceptualDuration,
                                       bounce: Self.enterSpring.bounce)
        spring.keyPath = "position"
        spring.fromValue = NSValue(point: from)
        spring.toValue = NSValue(point: to)
        spring.duration = spring.settlingDuration
        CATransaction.begin()
        CATransaction.setCompletionBlock { [weak self] in
            MainActor.assumeIsolated {
                guard let self, self.sliding == .entering else { return }
                self.sliding = .shown
                #if DEBUG
                self.applyDebugHooks()
                #endif
            }
        }
        root.container.layer?.add(spring, forKey: "slide")
        CATransaction.commit()
        startTicking()
    }

    public func slideOut() {
        guard sliding != .hidden, sliding != .leaving else { return }
        let from = root.container.layer?.presentation()?.position ?? root.container.layer?.position ?? .zero
        sliding = .leaving
        placeContent(hidden: true)
        let to = root.container.layer?.position ?? from
        let spring = CASpringAnimation(perceptualDuration: Self.exitSpring.perceptualDuration, bounce: Self.exitSpring.bounce)
        spring.keyPath = "position"
        spring.fromValue = NSValue(point: from)
        spring.toValue = NSValue(point: to)
        spring.duration = spring.settlingDuration
        CATransaction.begin()
        CATransaction.setCompletionBlock { [weak self] in
            MainActor.assumeIsolated {
                guard let self, self.sliding == .leaving else { return }
                self.hideNow()
            }
        }
        root.container.layer?.add(spring, forKey: "slide")
        CATransaction.commit()
    }

    public func setInteraction(_ interaction: SlotInteraction) {
        root.interaction = interaction
        if interaction == .none { panel.ignoresMouseEvents = true }
    }

    public func setKeyFocus(_ wanted: Bool) {
        if wanted {
            panel.allowKey = true
            if !panel.isKeyWindow {
                // Never NSApp.activate: the panel is non-activating and cooperative activation would be refused (§8.4).
                panel.makeKeyAndOrderFront(nil)
            }
        } else {
            let wasKey = panel.isKeyWindow
            panel.allowKey = false
            if wasKey { panel.resignKey() }
        }
    }

    // MARK: Internals

    private func hideNow() {
        sliding = .hidden
        panel.allowKey = false
        panel.orderOut(nil)
        panel.ignoresMouseEvents = true
        chrome.pointerMoved(to: nil, buttonDown: false)
        chrome.collapse()
        escapeKey.setActive(false)
        updateCursor()
        stopTicking()
    }

    #if DEBUG
    /// `PEEK_DEBUG_*` (see ``ChromeDebugHooks``): states that need a pointer or keys, for screenshots.
    private func applyDebugHooks() {
        guard !debugHooksApplied else { return }
        debugHooksApplied = true
        let hooks = chrome.debug
        if let text = hooks.type {
            chrome.actions.startTyping()
            let chrome = chrome
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) {
                chrome.actions.typingChanged(text)
                chrome.requestFieldFocus()  // the caret goes to the end, as after real typing
            }
        }
        chrome.applyDebugExpansion()
        chrome.applyDebugDrag()
    }
    #endif

    /// Puts the content at rest (`hidden` false) or beyond the edge, without animation.
    private func placeContent(hidden: Bool) {
        let offset = chrome.slotLayout.hiddenOffset
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        root.container.setFrameOrigin(hidden ? NSPoint(x: offset.dx, y: offset.dy) : .zero)
        CATransaction.commit()
    }

    private func startTicking() {
        guard displayLink == nil else { return }
        let link = root.displayLink(target: self, selector: #selector(tick(_:)))
        link.preferredFrameRateRange = CAFrameRateRange(minimum: 30, maximum: 60, preferred: 60)
        link.add(to: .main, forMode: .common)
        displayLink = link
    }

    private func stopTicking() {
        displayLink?.invalidate()
        displayLink = nil
    }

    @objc private func tick(_ link: CADisplayLink) {
        updateMousePassThrough()
        onTick?()
    }

    /// Click-through, hover and the cursor, from the polled pointer: tracking areas and `.onHover` do not fire in a
    /// panel that is never key in an app that is never active.
    private func updateMousePassThrough() {
        let mouse = NSEvent.mouseLocation
        let frame = panel.frame
        let point = CGPoint(x: mouse.x - frame.minX, y: frame.maxY - mouse.y)
        let buttonDown = NSEvent.pressedMouseButtons & 1 != 0
        var accept = root.isOverContent(point)
        // Keep a drag that left the control alive until the button is released.
        if !accept, !panel.ignoresMouseEvents, dragging || NSEvent.pressedMouseButtons != 0 { accept = root.interaction != .none }
        if panel.ignoresMouseEvents == accept { panel.ignoresMouseEvents = !accept }
        let interactive = root.interaction == .full && sliding != .leaving
        chrome.pointerMoved(to: interactive ? point : nil, buttonDown: buttonDown)
        if !buttonDown, chrome.pressed != nil { chrome.setPressed(nil) }
        updateCursor()
    }

    /// The pointing hand over anything clickable (#2). Set on every tick while it applies: the window server hands
    /// the cursor back to the frontmost app whenever that app sets it.
    private func updateCursor() {
        let wantsHand = chrome.hovered.map { chrome.isInteractive($0) } ?? false
        if wantsHand, !showsPointingHand { _ = BackgroundCursor.isEnabled }
        if wantsHand {
            NSCursor.pointingHand.set()
            showsPointingHand = true
        } else if showsPointingHand {
            NSCursor.arrow.set()
            showsPointingHand = false
        }
    }

    private func handleKey(_ event: NSEvent) -> Bool {
        if event.keyCode == EscapeHotKey.escapeKeyCode, chrome.expanded != nil {
            chrome.collapse()
            return true
        }
        let modifiers = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        let composing = (panel.firstResponder as? NSTextView)?.hasMarkedText() ?? false
        let input = KeyInput(
            characters: event.characters, charactersIgnoringModifiers: event.charactersIgnoringModifiers,
            keyCode: event.keyCode, commandOrControl: modifiers.contains(.command) || modifiers.contains(.control),
            composing: composing)
        return onKey?(input) ?? false
    }
}
