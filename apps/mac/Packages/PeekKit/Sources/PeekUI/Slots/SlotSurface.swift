import AppKit
import PeekCore

/// A key press, reduced to what ``KeyClassifier`` needs (so the manager can be tested without NSEvent).
public struct KeyInput: Sendable, Equatable {
    public var characters: String?
    public var charactersIgnoringModifiers: String?
    public var keyCode: UInt16
    public var commandOrControl: Bool
    /// An input method has marked (uncommitted) text in the typing field.
    public var composing: Bool

    public init(characters: String?, charactersIgnoringModifiers: String?, keyCode: UInt16, commandOrControl: Bool = false,
                composing: Bool = false) {
        self.characters = characters
        self.charactersIgnoringModifiers = charactersIgnoringModifiers
        self.keyCode = keyCode
        self.commandOrControl = commandOrControl
        self.composing = composing
    }
}

/// Which parts of the panel take the mouse.
public enum SlotInteraction: Sendable, Equatable {
    /// Everything passes through (hidden, sliding in).
    case none
    /// Only the down-arrow (the second click of a double click while the bubble slides out).
    case downArrowOnly
    /// The chrome, the down-arrow and the drawing's own pixels (B9).
    case full
}

/// One slot's on-screen presence: the panel with its visual view and chrome. The live
/// implementation is ``SlotPanelController``; tests and GUI-less runs use ``HeadlessSlotSurface``.
@MainActor
public protocol SlotSurface: AnyObject {
    var chrome: SlotChromeModel { get }
    /// The 100 × 100 unit square the drawing host attaches to.
    var visualView: NSView { get }
    var isKeyFocused: Bool { get }
    /// A key press while the panel is key. Return true when handled.
    var onKey: ((KeyInput) -> Bool)? { get set }
    var onResignKey: (() -> Void)? { get set }
    /// A click on the visual, in drawing units, with the click count.
    var onVisualClick: ((CGPoint, Int) -> Void)? { get set }
    /// A click on the down-arrow, with the click count (2 = double click).
    var onDownArrowClick: ((Int) -> Void)? { get set }
    /// Called on every frame while the panel is on screen (pointer polling, levels).
    var onTick: (() -> Void)? { get set }
    /// B9 for the drawing: is this unit point over a drawn pixel or glass?
    var visualHitTest: ((CGPoint) -> Bool)? { get set }

    /// Sizes and places the panel for `layout`. Only called while the panel is hidden.
    func configure(layout: SlotLayout)
    /// Shows the panel and springs the content in from beyond the screen edge.
    func slideIn()
    /// Slides the content back out and hides the panel once it is gone.
    func slideOut()
    func setInteraction(_ interaction: SlotInteraction)
    /// Allows (and takes) or gives up key focus, never activating the app.
    func setKeyFocus(_ wanted: Bool)
}

/// A surface without windows: keeps the chrome model (so layouts are computed) and records calls.
/// Used by tests and whenever Peek runs without a GUI session (`swift test`, no NSApp running).
@MainActor
public final class HeadlessSlotSurface: SlotSurface {
    public let chrome: SlotChromeModel
    public let visualView = NSView(frame: .zero)
    public private(set) var isKeyFocused = false
    public var onKey: ((KeyInput) -> Bool)?
    public var onResignKey: (() -> Void)?
    public var onVisualClick: ((CGPoint, Int) -> Void)?
    public var onDownArrowClick: ((Int) -> Void)?
    public var onTick: (() -> Void)? {
        didSet { updateTicking() }
    }
    public var visualHitTest: ((CGPoint) -> Bool)?

    public private(set) var layout: SlotLayout?
    public private(set) var slideIns = 0
    public private(set) var slideOuts = 0
    public private(set) var interaction: SlotInteraction = .none
    public private(set) var isOnScreen = false
    private var ticker: Task<Void, Never>?

    public init(chrome: SlotChromeModel) {
        self.chrome = chrome
    }

    public func configure(layout: SlotLayout) {
        self.layout = layout
        visualView.frame = layout.visualRect
        chrome.setSlotLayout(layout)
    }

    public func slideIn() {
        slideIns += 1
        isOnScreen = true
        updateTicking()
    }

    public func slideOut() {
        slideOuts += 1
        isOnScreen = false
        updateTicking()
    }

    public func setInteraction(_ interaction: SlotInteraction) { self.interaction = interaction }

    public func setKeyFocus(_ wanted: Bool) {
        let was = isKeyFocused
        isKeyFocused = wanted
        if was && !wanted { onResignKey?() }
    }

    /// Simulates a key press (tests).
    @discardableResult
    public func press(_ key: KeyInput) -> Bool { onKey?(key) ?? false }

    /// A 20 Hz tick while "on screen", so level polling works without a display link.
    private func updateTicking() {
        if isOnScreen, onTick != nil {
            guard ticker == nil else { return }
            ticker = Task { [weak self] in
                while !Task.isCancelled {
                    try? await Task.sleep(for: .milliseconds(50))
                    guard let self, !Task.isCancelled else { return }
                    self.onTick?()
                }
            }
        } else {
            ticker?.cancel()
            ticker = nil
        }
    }
}
