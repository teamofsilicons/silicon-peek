import CoreGraphics
import Foundation
import Observation
import PeekCore
import SwiftUI

/// Pill and band shading, chosen from what the bubble sits on so it stands out (understanding.md
/// "Style": shades of white and black picked for the backdrop; §8.5 ink is the opposite of tone).
/// Over a light backdrop pills are dark with white text (peek in-pactice.jpg); over a dark one they
/// are light with black text.
public struct PillShade: Sendable, Equatable {
    public enum Tone: Sendable, Equatable {
        /// Dark pills, white ink.
        case dark
        /// Light pills, black ink.
        case light
    }

    public var tone: Tone

    public init(tone: Tone) { self.tone = tone }

    /// Dark pills over light backdrops and light pills over dark ones.
    public static func forBackdrop(_ backdrop: Backdrop) -> PillShade {
        PillShade(tone: backdrop.tone == .light ? .dark : .light)
    }

    public static let darkPills = PillShade(tone: .dark)
    public static let lightPills = PillShade(tone: .light)

    /// Glass tint behind text.
    public var tint: Color { tone == .dark ? Color.black.opacity(0.52) : Color.white.opacity(0.62) }
    /// A stronger fill for selected options and the filled part of a slider track.
    public var strongFill: Color { tone == .dark ? Color.black.opacity(0.78) : Color.white.opacity(0.9) }
    /// Text and glyphs on the pills.
    public var ink: Color { tone == .dark ? .white : .black }
    public var secondaryInk: Color { ink.opacity(0.7) }
    /// Image borders.
    public var border: Color { tone == .dark ? Color(white: 0.16).opacity(0.9) : Color(white: 0.96).opacity(0.9) }
    /// The ink as a CGColor (the question arc draws with Core Text).
    public var inkCGColor: CGColor { tone == .dark ? CGColor(gray: 1, alpha: 1) : CGColor(gray: 0, alpha: 1) }
}

/// What the chrome calls back into the slot manager.
@MainActor
public struct ChromeActions {
    /// A tap on an option: answers a single choice, toggles a multiple choice.
    public var option: (String) -> Void = { _ in }
    /// The slider/range value while dragging.
    public var setValue: (AskValue) -> Void = { _ in }
    /// The ✓ button.
    public var submitValue: () -> Void = {}
    public var mic: () -> Void = {}
    public var keyboard: () -> Void = {}
    /// The text ask's "type your answer" prompt.
    public var startTyping: () -> Void = {}
    public var typingChanged: (String) -> Void = { _ in }
    public var submitTyping: () -> Void = {}
    /// A drag on a control started or ended (the panel keeps accepting the mouse meanwhile).
    public var dragging: (Bool) -> Void = { _ in }
    /// The pointer entered or left the chrome, or a popup opened or closed (the show's auto-dismiss pauses).
    public var hovering: (Bool) -> Void = { _ in }
    /// The `^` (or a click on the question) of a compact ask: expand it.
    public var expand: () -> Void = {}

    public init() {}
}

/// Everything one slot panel's chrome renders. The slot manager writes it; the SwiftUI views read it.
@MainActor
@Observable
public final class SlotChromeModel {
    public private(set) var slotLayout: SlotLayout
    public private(set) var content = ChromeContent()
    /// The computed layout for `slotLayout` + `content`.
    public private(set) var layout: ChromeLayout
    public private(set) var images: [String: CGImage] = [:]
    public private(set) var askPayload: AskPayload?
    public var askValue: AskValue?
    public var highlight: String?
    public private(set) var typingText = ""
    public private(set) var phase: Phase = .hidden
    public private(set) var shade = PillShade.darkPills
    public private(set) var context: InputContext = .production
    public private(set) var environmentTooltip: String?
    public private(set) var glassMode: GlassMode = .live
    /// Recent mic levels, oldest first (the live waveform).
    public private(set) var micLevels: [Double] = Array(repeating: 0, count: SlotChromeModel.waveformSamples)
    public private(set) var confirmEnabled = false
    /// Bumped to ask the typing field to take focus.
    public private(set) var focusRequest = 0
    /// The chrome is on screen (between slide-in and the end of the slide-out).
    public private(set) var isPresented = false

    // Pointer (ui-feedback.md #1–#3, #5), fed from the panel's frame tick.
    /// The piece of chrome under the pointer.
    public private(set) var hovered: ChromeTarget?
    /// A piece of chrome held down by the mouse, for the parts AppKit handles itself (the down-arrow).
    public private(set) var pressed: ChromeTarget?
    /// The static text whose tap-to-expand popup is open.
    public private(set) var expanded: ChromeTarget?
    /// The popup opened or closed (the panel then handles Esc).
    @ObservationIgnored public var onExpandedChanged: ((Bool) -> Void)?
    @ObservationIgnored private var buttonWasDown = false
    @ObservationIgnored private var pointerInside = false
    @ObservationIgnored let debug = ChromeDebugHooks.current

    @ObservationIgnored public var actions = ChromeActions()
    @ObservationIgnored public let measurer: any TextMeasuring

    public static let waveformSamples = 64

    public init(slotLayout: SlotLayout, measurer: any TextMeasuring = SystemTextMeasurer()) {
        self.slotLayout = slotLayout
        self.measurer = measurer
        layout = ChromeLayout.compute(layout: slotLayout, content: ChromeContent(), measurer: measurer)
    }

    // MARK: Writes from the slot manager

    public func setSlotLayout(_ newLayout: SlotLayout) {
        guard newLayout != slotLayout else { return }
        slotLayout = newLayout
        relayout()
    }

    public func setContent(_ newContent: ChromeContent, askPayload: AskPayload?) {
        if newContent != content || askPayload != self.askPayload {
            content = newContent
            self.askPayload = askPayload
            relayout()
        }
    }

    public func setImages(_ newImages: [String: CGImage]) {
        images = newImages
    }

    public func setTypingText(_ text: String) {
        if typingText != text { typingText = text }
    }

    public func setPhase(_ newPhase: Phase) {
        if phase != newPhase { phase = newPhase }
        let presented = newPhase != .hidden
        if isPresented != presented {
            isPresented = presented
            if !presented {
                collapse()
                if hovered != nil { hovered = nil }
                if pressed != nil { pressed = nil }
            }
        }
    }

    public func setShade(_ newShade: PillShade) {
        if shade != newShade { shade = newShade }
    }

    public func setContext(_ newContext: InputContext, tooltip: String?, glass: GlassMode) {
        if context != newContext { context = newContext }
        if environmentTooltip != tooltip { environmentTooltip = tooltip }
        if glassMode != glass { glassMode = glass }
    }

    public func setConfirmEnabled(_ enabled: Bool) {
        if confirmEnabled != enabled { confirmEnabled = enabled }
    }

    public func requestFieldFocus() { focusRequest &+= 1 }

    /// Appends the current mic level to the waveform history (called from the panel's frame tick).
    public func pushMicLevel(_ level: Double) {
        micLevels.removeFirst()
        micLevels.append(min(max(level, 0), 1))
    }

    public func resetMicLevels() {
        if micLevels.contains(where: { $0 != 0 }) {
            micLevels = Array(repeating: 0, count: Self.waveformSamples)
        }
    }

    // MARK: Reads for the views

    public func image(for key: String?) -> CGImage? { key.flatMap { images[$0] } }

    /// The option with this index in the current ask.
    public func option(at index: Int) -> ChromeContent.Option? {
        guard case .choice(let options, _)? = content.controls, options.indices.contains(index) else { return nil }
        return options[index]
    }

    public func isSelected(_ optionID: String) -> Bool {
        switch askValue {
        case .choice(let id)?: id == optionID
        case .choices(let ids)?: ids.contains(optionID)
        default: false
        }
    }

    /// Slider/range bounds, step and unit.
    public var scaleSpec: (min: Double, max: Double, step: Double, unit: String?)? {
        switch askPayload?.kind {
        case .slider(let spec)?: (spec.min, spec.max, spec.step, spec.unit)
        case .range(let spec)?: (spec.min, spec.max, spec.step, spec.unit)
        default: nil
        }
    }

    /// A value formatted on the ask's step grid, with its unit.
    public func format(_ value: Double) -> String {
        guard let spec = scaleSpec else { return String(value) }
        let places = TypedAnswerMatcher.decimalPlaces(of: spec.step)
        return Self.format(value, places: places, unit: spec.unit)
    }

    /// `42%`, `$120`, `3.5 km`: currency symbols lead, `%` and `°` attach, other units follow after a space.
    public nonisolated static func format(_ value: Double, places: Int, unit: String?) -> String {
        let number = String(format: "%.\(places)f", value)
        guard let unit, !unit.isEmpty else { return number }
        if unit == "%" || unit == "°" { return number + unit }
        let isCurrency = unit.unicodeScalars.count == 1 && unit.unicodeScalars.first?.properties.generalCategory == .currencySymbol
        if isCurrency { return value < 0 ? "-\(unit)\(number.dropFirst())" : unit + number }
        return "\(number) \(unit)"
    }

    public var fieldPlaceholder: String {
        if case .text(let placeholder)? = content.controls, let placeholder, !placeholder.isEmpty { return placeholder }
        switch content.controls {
        case .text?: return "Type your answer…"
        case .choice?, .scale?: return "Type an option…"
        case nil: return "Message…"
        }
    }

    private func relayout() {
        let next = ChromeLayout.compute(layout: slotLayout, content: content, measurer: measurer)
        if next != layout { layout = next }
        if let expanded, !layout.isExpandable(expanded) { collapse() }
    }

    // MARK: Pointer, hover and the reveal overlays

    /// Whether `target` does something when clicked: those look (and react) differently from static chrome (#2, #3).
    public func isInteractive(_ target: ChromeTarget) -> Bool {
        switch target {
        case .mic, .keyboard, .down, .field, .submit, .track, .popup, .expand: return true
        // On a compact ask a click on the question expands the ask.
        case .question: return content.askCollapsed || layout.isExpandable(.question)
        case .item(let identity):
            if identity.hasPrefix("option-") || identity == "field" { return true }
            if identity == "confirm" { return confirmEnabled }
            return layout.isExpandable(target)
        }
    }

    /// Hover (and press) state for drawing `target`: only interactive chrome reacts.
    public func isHovered(_ target: ChromeTarget) -> Bool { hovered == target && isInteractive(target) }
    public func isPressed(_ target: ChromeTarget) -> Bool { pressed == target || debugPressed == target }

    /// The panel's frame tick: the pointer (panel-local; nil when the panel does not take the pointer at all) and
    /// whether a mouse button is down anywhere. Updates the hover, closes the popup on a click outside it, and
    /// tells the bubble when the pointer enters or leaves.
    public func pointerMoved(to point: CGPoint?, buttonDown: Bool) {
        var target: ChromeTarget?
        if isPresented, let point {
            if let popup = popupOverlay, popup.frame.contains(point) {
                target = .popup
            } else {
                target = layout.target(at: point)
            }
        }
        let shown = debugHovered ?? target
        if hovered != shown { hovered = shown }
        if buttonDown, !buttonWasDown, expanded != nil, target != .popup, target != expanded { collapse() }
        buttonWasDown = buttonDown
        let inside = target != nil || expanded != nil
        if inside != pointerInside {
            pointerInside = inside
            actions.hovering(inside)
        }
    }

    public func setPressed(_ target: ChromeTarget?) {
        if pressed != target { pressed = target }
    }

    /// A click on the question: expands a compact ask, else toggles the question's popup when it is cut short.
    public func questionClicked() {
        if content.askCollapsed {
            collapse()
            actions.expand()
        } else {
            toggleExpanded(.question)
        }
    }

    /// A click on expandable text: opens its popup, or closes it when it is already open (#5).
    public func toggleExpanded(_ target: ChromeTarget) {
        if expanded == target || target == .popup {
            collapse()
        } else if layout.isExpandable(target) {
            expanded = target
            onExpandedChanged?(true)
            if !pointerInside {
                pointerInside = true
                actions.hovering(true)
            }
        }
    }

    /// Closes the popup (Esc, a click outside, a new layout).
    public func collapse() {
        guard expanded != nil else { return }
        expanded = nil
        onExpandedChanged?(false)
    }

    /// The open popup, if any. (Memoized: the panel asks on every frame tick for hit testing.)
    public var popupOverlay: ChromeOverlay? {
        guard let expanded else { return nil }
        if let cached = popupCache, cached.target == expanded, cached.layout == layout { return cached.overlay }
        let overlay = layout.hiddenText(of: expanded).flatMap {
            ChromeOverlay.make(.popup, for: expanded, text: $0, layout: layout, measurer: measurer)
        }
        popupCache = (expanded, layout, overlay)
        return overlay
    }

    /// The hover tooltip with the complete text of whatever cut-short text is under the pointer.
    public var tooltipOverlay: ChromeOverlay? {
        guard expanded == nil, let hovered, let text = layout.hiddenText(of: hovered) else { return nil }
        return ChromeOverlay.make(.tooltip, for: hovered, text: text, layout: layout, measurer: measurer)
    }

    @ObservationIgnored private var popupCache: (target: ChromeTarget, layout: ChromeLayout, overlay: ChromeOverlay?)?

    // MARK: Debug hooks (screenshots)

    private var debugHovered: ChromeTarget? { debug.hover.flatMap(resolve) }
    private var debugPressed: ChromeTarget? { debug.press.flatMap(resolve) }

    /// Applies `PEEK_DEBUG_EXPAND` (Debug builds): opens that target's popup once the bubble is up.
    public func applyDebugExpansion() {
        guard let spec = debug.expand, let target = resolve(spec) else { return }
        toggleExpanded(target)
    }

    /// `PEEK_DEBUG_DRAG`, handed to the slider once the bubble is up (its answer value only changes when visible).
    public private(set) var debugDrag: Double?

    public func applyDebugDrag() {
        if let drag = debug.drag { debugDrag = drag }
    }

    /// "option-1", "text-0", "question", "mic", "field" …: a target of the current layout.
    func resolve(_ spec: String) -> ChromeTarget? {
        switch spec {
        case "question": return .question
        case "field": return layout.field != nil ? .field : .item("field")
        case "submit": return .submit
        case "track": return .track
        case "mic": return .mic
        case "keyboard": return .keyboard
        case "down": return .down
        case "expand": return layout.buttons.expand != nil ? .expand : nil
        case "popup": return .popup
        default: return layout.item(identity: spec) != nil ? .item(spec) : nil
        }
    }
}

/// Environment hooks that force hover, press, expansion, a mid-drag slider or typed text, so screenshots can show
/// states that need a pointer. Debug builds only; Release builds ignore the variables.
///
///     PEEK_DEBUG_HOVER=option-1   PEEK_DEBUG_PRESS=mic   PEEK_DEBUG_EXPAND=image-0
///     PEEK_DEBUG_DRAG=0.43        PEEK_DEBUG_TYPE="Late-night focus"
public struct ChromeDebugHooks: Sendable, Equatable {
    public var hover: String?
    public var press: String?
    public var expand: String?
    /// A slider/range thumb held mid-drag at this fraction of the track.
    public var drag: Double?
    /// Start typing this text once the bubble is up.
    public var type: String?

    public static let current: ChromeDebugHooks = {
        #if DEBUG
        let env = ProcessInfo.processInfo.environment
        func value(_ key: String) -> String? { env[key].flatMap { $0.isEmpty ? nil : $0 } }
        return ChromeDebugHooks(hover: value("PEEK_DEBUG_HOVER"), press: value("PEEK_DEBUG_PRESS"),
                                expand: value("PEEK_DEBUG_EXPAND"), drag: value("PEEK_DEBUG_DRAG").flatMap(Double.init),
                                type: value("PEEK_DEBUG_TYPE"))
        #else
        return ChromeDebugHooks()
        #endif
    }()

    public var isEmpty: Bool { self == ChromeDebugHooks() }
}
