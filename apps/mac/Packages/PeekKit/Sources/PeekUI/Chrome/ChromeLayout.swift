import CoreGraphics
import Foundation
import PeekCore

// The chrome's layout, as a pure function of the slot geometry (PeekCore's `SlotLayout`) and
// what the bubble shows. It decides where every pill, image, option, control, the question
// arc and the mic/keyboard/down-arrow buttons go, and which regions catch the pointer.
// The SwiftUI views only draw what this computes, so tests can check the layout without AppKit.
//
// Normal mode (understanding.md "Visuals", visual.md B2, BLUEPRINT §8.5):
//   * one row of items sits on the outer side of the information arc's baseline, flex-centred in
//     reading order; each show element is at most a third of the arc (`InfoArc.maxElementAlongExtent`);
//     pills and images rotate to the arc's tangent (the rotated "2022" pill in peek in-pactice.jpg);
//   * slider/range tracks and the live waveform take the whole arc;
//   * the question is its own arc further out, set glyph by glyph on a concentric circle;
//   * the mic, keyboard and down-arrow buttons sit in the visual square's outer corners
//     ("below" the visual, between it and the screen edge it hides behind).
// User UI feedback (ui-feedback.md, 2026-09-26, overrides the blueprint):
//   * long text wraps into a narrow, tall column (``TextColumn``) that grows away from the screen edge,
//     tilts at most ``ChromeMetrics/columnMaxTilt`` off upright and is cut to stay inside the panel;
//     anything cut short is flagged (`truncated`) so the chrome can reveal it on hover and on click;
//   * the typing field of a text ask is a curved band (``ChromeLayout/CurvedField``) right next to the
//     question band, following the same arc (upright blocks stacked question-over-field on the left/right arcs);
//   * nothing is drawn behind the elements: no panel, scrim or strip background.
// Compact mode: the small visual on the left, the question on top, the row below it and the buttons in a
// column at the right end. `stripRect` is only the region they are laid out in; it is not drawn.

/// What the chrome shows. Built by the slot manager from the bubble's state.
public struct ChromeContent: Sendable, Equatable {
    public enum Badge: Sendable, Equatable {
        /// `SIMULATION` (§8.11).
        case simulation

        public var text: String {
            switch self {
            case .simulation: "SIMULATION"
            }
        }
    }

    public enum Element: Sendable, Equatable {
        case text(String)
        /// `key` is the image's cache path; `aspect` is width / height (1 when the image could not be read).
        case image(key: String, aspect: CGFloat, caption: String?)
    }

    public struct Option: Sendable, Equatable, Identifiable {
        public var id: String
        public var label: String
        public var imageKey: String?
        public var aspect: CGFloat

        public init(id: String, label: String, imageKey: String? = nil, aspect: CGFloat = 1) {
            self.id = id
            self.label = label
            self.imageKey = imageKey
            self.aspect = aspect
        }
    }

    public enum Controls: Sendable, Equatable {
        case text(placeholder: String?)
        case choice(options: [Option], multiple: Bool)
        /// Slider (`isRange` false) or range: the track uses the whole arc.
        case scale(isRange: Bool)
    }

    public enum Input: Sendable, Equatable {
        case none
        case typing
        case listening
        case transcribing
    }

    public var badge: Badge?
    public var elements: [Element]
    public var question: String?
    public var controls: Controls?
    public var input: Input
    public var notice: String?
    /// A short hint for a summoned bubble with nothing to show ("\ to talk · type to write").
    public var hint: String?
    /// Sends of this Silicon waiting behind the bubble: the "+N" badge next to the down-arrow (0 = none).
    public var waiting: Int
    /// The ask is collapsed to the compact ask: the controls row is hidden, the question and `^` stay.
    public var askCollapsed: Bool
    /// "Esc again to dismiss" shows where the controls row was.
    public var escHint: Bool

    public init(badge: Badge? = nil, elements: [Element] = [], question: String? = nil, controls: Controls? = nil,
                input: Input = .none, notice: String? = nil, hint: String? = nil, waiting: Int = 0,
                askCollapsed: Bool = false, escHint: Bool = false) {
        self.badge = badge
        self.elements = elements
        self.question = question
        self.controls = controls
        self.input = input
        self.notice = notice
        self.hint = hint
        self.waiting = waiting
        self.askCollapsed = askCollapsed
        self.escHint = escHint
    }

    /// Nothing at all on the arc (the buttons still show).
    public var isEmpty: Bool {
        badge == nil && elements.isEmpty && question == nil && controls == nil && input == .none && notice == nil
            && hint == nil && !escHint
    }

    /// The controls row as laid out: hidden on a compact ask.
    var visibleControls: Controls? { askCollapsed ? nil : controls }

    /// The text of the row's hint pill: "Esc again to dismiss" on a compact ask, else the summon hint.
    public var hintText: String? { escHint ? Self.escAgainHint : hint }

    public static let escAgainHint = "Esc again to dismiss"
}

/// Fixed sizes of the chrome (points).
public enum ChromeMetrics {
    public static let pillHeight: CGFloat = 24
    public static let pillPadding: CGFloat = 10
    public static let pillVerticalPadding: CGFloat = 5
    public static let pillCornerRadius: CGFloat = 13
    /// Long text grows tall rather than wide; the room left in the panel usually limits it first.
    public static let pillMaxLines = 16
    public static let captionHeight: CGFloat = SlotGeometry.pillHeight
    public static let captionGap: CGFloat = 4
    public static let badgeHeight: CGFloat = 20
    public static let optionHeight: CGFloat = 28
    public static let optionPadding: CGFloat = 12
    /// Side padding of a multiple-choice option, and the width of its check circle plus spacing.
    public static let multipleOptionPadding: CGFloat = 9
    public static let multipleOptionIcon: CGFloat = 18
    public static let confirmSize: CGFloat = 30
    public static let fieldHeight: CGFloat = 32
    public static let fieldPreferredWidth: CGFloat = 340
    public static let fieldMinWidth: CGFloat = 140
    /// The upright typing field on the diagonal corner arcs.
    public static let cornerFieldWidth: CGFloat = 250
    public static let waveformThickness: CGFloat = 36
    public static let thumbSize: CGFloat = 22
    public static let trackThickness: CGFloat = 6
    public static let valuePillGap: CGFloat = 4
    public static let questionPadding: CGFloat = 5
    public static let questionCapPadding: CGFloat = 8
    public static let minPillAlong: CGFloat = 40
    public static let minImageAlong: CGFloat = 28
    public static let minSpanAlong: CGFloat = 96
    /// Angle between the down-arrow and each of the mic/keyboard buttons.
    public static let buttonSpread: Double = 50 * .pi / 180
    public static let compactButtonRadius: CGFloat = 11
    /// Content inset from the start of compact mode's line.
    public static let compactInset: CGFloat = 8
    public static let compactCornerRadius: CGFloat = 22

    // Long text: narrow and tall (ui-feedback.md #4).
    /// Text up to this wide (one line) stays a single-line pill.
    public static let columnShortWidth: CGFloat = 200
    public static let columnMinWidth: CGFloat = 110
    public static let columnMaxWidth: CGFloat = 190
    /// Target column height ÷ width (before balancing): 1 ≈ square text block, which reads as a tall pill.
    public static let columnAspect: CGFloat = 1
    /// A multi-line column tilts at most this far off upright, however steep the arc is there.
    public static let columnMaxTilt: Double = 15 * .pi / 180
    /// Items stay this far inside the panel's edges.
    public static let panelInset: CGFloat = 4

    // The curved typing field (ui-feedback.md #7).
    public static let fieldBandThickness: CGFloat = 30
    public static let fieldBandPreferredLength: CGFloat = 360
    public static let fieldSubmitRadius: CGFloat = 11
    /// Between the field band and the question band.
    public static let fieldQuestionGap: CGFloat = 4

    // Overlays (ui-feedback.md #1, #5).
    public static let tooltipMaxWidth: CGFloat = 250
    public static let popupMaxWidth: CGFloat = 280
    public static let overlayPadding: CGFloat = 10

    // The "+N" badge and the `^` expand button next to the down-arrow (peek 0.1.2).
    /// Pushed outward in these steps (at most ``accessoryMaxPush``) until it clears the other buttons.
    public static let accessoryPushStep: CGFloat = 2
    public static let accessoryMaxPush: CGFloat = 12
    /// Space kept between an accessory and a neighbouring button.
    public static let accessoryClearance: CGFloat = 1
    public static let waitingBadgeFont = ChromeFont(size: 11, weight: .semibold)
}

/// The computed chrome layout (panel-local, y-down).
public struct ChromeLayout: Sendable, Equatable {
    public enum ItemKind: Sendable, Equatable {
        case badge
        /// A show text element (index into `ChromeContent.elements`) wrapped to `lines` lines.
        case textPill(element: Int, lines: Int)
        /// A show image with its caption pill underneath; `imageSize` is the image box inside the card.
        case imageCard(element: Int, imageSize: CGSize)
        /// An ask option: a label pill, or an image card when `imageSize` is set.
        case option(index: Int, imageSize: CGSize?)
        /// The typing field, or the "tap to type" prompt of a text ask.
        case field
        /// The ✓ that submits multiple-choice, slider and range answers.
        case confirm
        case hint
    }

    public struct Item: Sendable, Equatable {
        public var kind: ItemKind
        public var frame: RotatedRect
        /// Position along the arc (normal mode) or the line (compact mode), from the middle, in reading order.
        public var arcLength: CGFloat
        /// Its text (the pill's text, an image's caption, an option's label) is cut short with "…".
        public var truncated = false
        /// The complete text, shown on hover (and on click for static text) when `truncated`.
        public var fullText: String?
    }

    /// The typing field of a text ask as a curved glass band next to the question (ui-feedback.md #7).
    /// Text is set glyph by glyph on `arc` (the band's middle circle) from `textStart` to `textEnd`
    /// (arc lengths from its midpoint, reading order).
    public struct CurvedField: Sendable, Equatable {
        public var arc: InfoArc
        public var band: ArcBand
        public var halfLength: CGFloat
        public var thickness: CGFloat
        public var upIsOutward: Bool
        /// The keyboard glyph of the "type your answer" prompt.
        public var iconS: CGFloat
        public var textStart: CGFloat
        public var textEnd: CGFloat
        /// The send button at the reading end.
        public var submitS: CGFloat
        public var submitRadius: CGFloat

        public var submitCenter: CGPoint { arc.point(atArcLength: submitS) }
        public var iconCenter: CGPoint { arc.point(atArcLength: iconS) }
        public func rotation(at s: CGFloat) -> Double { arc.elementRotation(atArcLength: s) }
    }

    public enum TrackRole: Sendable, Equatable {
        case slider
        case range
        case waveform
    }

    /// The slider/range track or the waveform band: the whole remaining arc (or line).
    public enum Track: Sendable, Equatable {
        /// On the baseline arc from `s0` to `s1` (reading order); the centreline is `offset` points outward.
        case arc(InfoArc, s0: CGFloat, s1: CGFloat, offset: CGFloat, thickness: CGFloat)
        case line(start: CGPoint, end: CGPoint, thickness: CGFloat)

        public var thickness: CGFloat {
            switch self {
            case .arc(_, _, _, _, let thickness), .line(_, _, let thickness): thickness
            }
        }

        public var length: CGFloat {
            switch self {
            case .arc(_, let s0, let s1, _, _): abs(s1 - s0)
            case .line(let start, let end, _): hypot(end.x - start.x, end.y - start.y)
            }
        }

        /// The centreline point at fraction `t` (0 = reading start, 1 = end), `lift` points further outward.
        public func point(at t: Double, lift: CGFloat = 0) -> CGPoint {
            switch self {
            case .arc(let arc, let s0, let s1, let offset, _):
                return arc.point(atArcLength: s0 + (s1 - s0) * CGFloat(t), offset: offset + lift)
            case .line(let start, let end, _):
                return CGPoint(x: start.x + (end.x - start.x) * CGFloat(t), y: start.y + (end.y - start.y) * CGFloat(t) - lift)
            }
        }

        /// Rotation of an upright element sitting on the track at `t`.
        public func rotation(at t: Double) -> Double {
            switch self {
            case .arc(let arc, let s0, let s1, _, _):
                arc.elementAxisOffset == 0 ? arc.elementRotation(atArcLength: s0 + (s1 - s0) * CGFloat(t)) : 0
            case .line: 0
            }
        }

        /// The fraction (clamped to 0…1) of the track point nearest to `point`.
        public func fraction(nearest point: CGPoint) -> Double {
            switch self {
            case .arc(let arc, let s0, let s1, _, _):
                guard s1 != s0 else { return 0 }
                let s = arc.arcLength(nearest: point)
                return Double(min(max((s - s0) / (s1 - s0), 0), 1))
            case .line(let start, let end, _):
                let vx = end.x - start.x, vy = end.y - start.y
                let lengthSquared = vx * vx + vy * vy
                guard lengthSquared > 0 else { return 0 }
                return Double(min(max(((point.x - start.x) * vx + (point.y - start.y) * vy) / lengthSquared, 0), 1))
            }
        }

        /// The band `extra` points thicker than the track on each side.
        public func band(extra: CGFloat = 0) -> HitShape {
            switch self {
            case .arc(let arc, let s0, let s1, let offset, let thickness):
                .arc(arc.band(from: s0, to: s1, inner: offset - thickness / 2 - extra, outer: offset + thickness / 2 + extra))
            case .line(let start, let end, let thickness):
                .line(LineBand(start: start, end: end, thickness: thickness + 2 * extra))
            }
        }
    }

    public struct QuestionLine: Sendable, Equatable {
        public enum Placement: Sendable, Equatable {
            /// Glyphs follow `arc` (a concentric circle through the line's middle), centred on its midpoint.
            /// `upIsOutward`: glyph tops point away from the arc's centre (bottom slots) or toward it (top slots).
            case arc(InfoArc, upIsOutward: Bool)
            /// Upright straight text in this box (vertical arcs, compact mode); may wrap to `lines` lines.
            case block(RotatedRect, lines: Int)
        }

        public var text: String
        public var font: ChromeFont
        public var width: CGFloat
        public var placement: Placement
    }

    public enum QuestionBackground: Sendable, Equatable {
        case arc(ArcBand)
        case rect(RotatedRect)
    }

    public struct Question: Sendable, Equatable {
        public var lines: [QuestionLine]
        public var background: QuestionBackground
        /// The band shows a notice in place of the question.
        public var isNotice: Bool
        /// The lines are cut short with "…"; `fullText` is revealed on hover and on click.
        public var truncated = false
        public var fullText = ""
    }

    public struct Buttons: Sendable, Equatable {
        public var mic: CGPoint
        public var keyboard: CGPoint
        public var down: CGPoint
        public var radius: CGFloat
        public var downRadius: CGFloat
        /// Rotation of a `chevron.down` glyph so it points toward the edge the bubble slides behind.
        public var downRotation: Double
        /// The `^` that expands a compact ask (radius ``downRadius``); nil unless the ask is collapsed.
        public var expand: CGPoint?
        /// The "+N" badge's centre and size (an upright capsule); nil when nothing waits.
        public var badge: CGPoint?
        public var badgeSize: CGSize?

        public init(mic: CGPoint, keyboard: CGPoint, down: CGPoint, radius: CGFloat, downRadius: CGFloat, downRotation: Double,
                    expand: CGPoint? = nil, badge: CGPoint? = nil, badgeSize: CGSize? = nil) {
            self.mic = mic
            self.keyboard = keyboard
            self.down = down
            self.radius = radius
            self.downRadius = downRadius
            self.downRotation = downRotation
            self.expand = expand
            self.badge = badge
            self.badgeSize = badgeSize
        }

        /// The badge's upright box.
        public var badgeRect: CGRect? {
            guard let badge, let badgeSize else { return nil }
            return CGRect(x: badge.x - badgeSize.width / 2, y: badge.y - badgeSize.height / 2, width: badgeSize.width,
                          height: badgeSize.height)
        }

        /// The `^` glyph points away from the edge the bubble hides behind (the down-arrow's opposite).
        public var expandRotation: Double { downRotation }
    }

    public var mode: DisplayMode
    public var items: [Item]
    public var track: Track?
    public var trackRole: TrackRole?
    public var question: Question?
    public var buttons: Buttons
    /// Compact mode's content region (laid out in, never drawn: the peek has no background of its own).
    public var stripRect: CGRect?
    public var visualRect: CGRect
    public var hitShapes: [HitShape]
    /// The curved typing field (normal mode, top/bottom and corner arcs); a straight `.field` item otherwise.
    public var field: CurvedField?
    /// The panel's size, for keeping overlays inside it.
    public var panelSize: CGSize = .zero

    /// Whether `point` (panel-local) is over the chrome (B9), not counting the drawing.
    public func isOverChrome(_ point: CGPoint) -> Bool {
        hitShapes.contains { $0.contains(point) }
    }

    public func item(_ kind: ItemKind) -> Item? { items.first { $0.kind == kind } }

    /// Items of a given family, in order.
    public var optionItems: [Item] {
        items.filter {
            if case .option = $0.kind { return true }
            return false
        }
    }

    // MARK: - Computing

    /// Lays out `content` for `layout` (normal: arc; compact: strip).
    public static func compute(layout: SlotLayout, content: ChromeContent, measurer: any TextMeasuring) -> ChromeLayout {
        if let arc = layout.arc {
            return ArcChromeBuilder(layout: layout, arc: arc, content: content, measurer: measurer).build()
        }
        if let line = layout.line {
            return StripChromeBuilder(layout: layout, line: line, content: content, measurer: measurer).build()
        }
        return ChromeLayout(
            mode: layout.mode, items: [], track: nil, trackRole: nil, question: nil,
            buttons: Buttons(mic: .zero, keyboard: .zero, down: .zero, radius: 0, downRadius: 0, downRotation: 0),
            stripRect: nil, visualRect: layout.visualRect, hitShapes: [], panelSize: layout.panelSize)
    }

    /// The mic, keyboard and down-arrow buttons in the visual square's outer corners.
    static func cornerButtons(layout: SlotLayout, inwardAngle: Double) -> Buttons {
        let r = layout.visualRadius
        let radius = min(max(r * 0.17, 11), 15)
        let distance = r + radius + 3
        let outward = inwardAngle + .pi
        let c = layout.visualCenter
        func at(_ angle: Double, _ d: CGFloat) -> CGPoint {
            CGPoint(x: c.x + d * CGFloat(cos(angle)), y: c.y + d * CGFloat(sin(angle)))
        }
        let a = at(outward - ChromeMetrics.buttonSpread, distance)
        let b = at(outward + ChromeMetrics.buttonSpread, distance)
        // The mic comes first in reading order (left, or top when the two share a column).
        let aFirst = abs(a.x - b.x) > 0.5 ? a.x < b.x : a.y < b.y
        return Buttons(
            mic: aFirst ? a : b, keyboard: aFirst ? b : a, down: at(outward, distance), radius: radius,
            downRadius: (radius * 0.9).rounded(), downRotation: wrappedAngle(outward - .pi / 2))
    }

    /// The "+N" badge's size: a capsule `round(1.5 × downRadius)` tall, at least as wide as tall, fitting "+N".
    static func waitingBadgeSize(_ waiting: Int, downRadius: CGFloat, measurer: any TextMeasuring) -> CGSize {
        let height = (1.5 * downRadius).rounded()
        let text = measurer.width(of: "+\(waiting)", font: ChromeMetrics.waitingBadgeFont)
        return CGSize(width: max(height, (text + 0.75 * height).rounded(.up)), height: height)
    }

    /// Normal mode: `^` at the mid-angle between the down-arrow and the mic, the badge between the down-arrow and the
    /// keyboard, both at the buttons' distance from the visual's centre; each is pushed outward in 2 pt steps (at most
    /// 12 pt) while it would overlap another button. Near a tight panel edge (small visuals) it may also turn a few
    /// degrees along its circle, and as a last resort it is moved inside the panel.
    static func placeAccessories(_ buttons: inout Buttons, layout: SlotLayout, waiting: Int, collapsed: Bool,
                                 measurer: any TextMeasuring) {
        let c = layout.visualCenter
        let panel = CGRect(origin: .zero, size: layout.panelSize).insetBy(dx: 1, dy: 1)
        func polar(_ p: CGPoint) -> (angle: Double, distance: CGFloat) {
            (atan2(Double(p.y - c.y), Double(p.x - c.x)), hypot(p.x - c.x, p.y - c.y))
        }
        func midAngle(_ a: Double, _ b: Double) -> Double { a + wrappedAngle(b - a) / 2 }
        func at(_ angle: Double, _ d: CGFloat) -> CGPoint {
            CGPoint(x: c.x + d * CGFloat(cos(angle)), y: c.y + d * CGFloat(sin(angle)))
        }
        let down = polar(buttons.down)
        let distance = down.distance
        let circles: [(CGPoint, CGFloat)] = [(buttons.mic, buttons.radius), (buttons.keyboard, buttons.radius),
                                             (buttons.down, buttons.downRadius)]
        func clear(_ rect: CGRect, round: Bool) -> Bool {
            circles.allSatisfy { circle in
                if round {
                    let center = CGPoint(x: rect.midX, y: rect.midY)
                    return hypot(circle.0.x - center.x, circle.0.y - center.y)
                        >= circle.1 + rect.width / 2 + ChromeMetrics.accessoryClearance
                }
                let nearest = CGPoint(x: min(max(circle.0.x, rect.minX), rect.maxX), y: min(max(circle.0.y, rect.minY), rect.maxY))
                return hypot(nearest.x - circle.0.x, nearest.y - circle.0.y) >= circle.1 + ChromeMetrics.accessoryClearance
            }
        }
        /// The first position (outward push first, then small turns) clear of the buttons and inside the panel.
        func place(size: CGSize, angle: Double, round: Bool) -> CGPoint {
            func rect(_ center: CGPoint) -> CGRect {
                CGRect(x: center.x - size.width / 2, y: center.y - size.height / 2, width: size.width, height: size.height)
            }
            let turns = [0.0, 4, -4, 8, -8, 12, -12].map { $0 * .pi / 180 }
            var firstClear: CGPoint?
            for turn in turns {
                var push: CGFloat = 0
                while push <= ChromeMetrics.accessoryMaxPush {
                    let point = at(angle + turn, distance + push)
                    if clear(rect(point), round: round) {
                        if panel.contains(rect(point)) { return point }
                        if firstClear == nil { firstClear = point }
                        break
                    }
                    push += ChromeMetrics.accessoryPushStep
                }
            }
            let fallback = firstClear ?? at(angle, distance + ChromeMetrics.accessoryMaxPush)
            let clamped = ChromeOverlay.clamp(rect(fallback), into: panel)
            return CGPoint(x: clamped.midX, y: clamped.midY)
        }
        if collapsed {
            let d = 2 * buttons.downRadius
            buttons.expand = place(size: CGSize(width: d, height: d), angle: midAngle(down.angle, polar(buttons.mic).angle),
                                   round: true)
        }
        if waiting > 0 {
            let size = waitingBadgeSize(waiting, downRadius: buttons.downRadius, measurer: measurer)
            buttons.badge = place(size: size, angle: midAngle(down.angle, polar(buttons.keyboard).angle), round: false)
            buttons.badgeSize = size
        }
    }
}

// MARK: - Row sizing and fitting (shared by both builders)

/// How a row item sizes itself for an along-the-row limit.
enum RowSizing {
    /// Never shrinks.
    case fixed(CGSize)
    /// An image (`aspect` = w/h) inside a box of `maxSide`, with `extraHeight` below it (a caption) and a
    /// minimum card width for the caption.
    case image(aspect: CGFloat, maxSide: CGFloat, extraHeight: CGFloat, captionWidth: CGFloat)
    /// Wrapping text in a rounded pill.
    case text(String, ChromeFont, maxLines: Int)
    /// One line, truncated with "…" when it does not fit.
    case singleLine(String, ChromeFont, height: CGFloat, padding: CGFloat)
    /// Takes whatever the row has left (slider/range track, waveform), never less than `minAlong`.
    case span(minAlong: CGFloat, depth: CGFloat)

    var isSpan: Bool {
        if case .span = self { return true }
        return false
    }

    var isShrinkable: Bool {
        switch self {
        case .image, .text, .singleLine: true
        case .fixed, .span: false
        }
    }

    var minAlong: CGFloat {
        switch self {
        case .image: ChromeMetrics.minImageAlong
        case .text, .singleLine: ChromeMetrics.minPillAlong
        case .fixed(let size): size.width
        case .span(let minAlong, _): minAlong
        }
    }
}

struct RowRequest {
    var kind: ChromeLayout.ItemKind
    var sizing: RowSizing
    /// Show elements, options and the badge are capped at a third of the row; the field and ✓ are not.
    var capToThird: Bool
    /// The complete text of the item (revealed when it is cut short).
    var fullText: String? = nil
    /// A stand-in for the upright question block stacked above the field on the left/right arcs.
    var isQuestionBlock = false
}

struct RowSize {
    var size: CGSize
    var along: CGFloat
    var depth: CGFloat
    /// Lines, for wrapping text pills.
    var lines: Int
    /// The image box inside a card.
    var imageSize: CGSize?
    /// The text (or caption) does not fit and is cut with "…".
    var truncated = false
}

/// A block of wrapped text sized "narrow and tall" (ui-feedback.md #4): short text stays on one line; longer text
/// wraps at a width near √(text width × line height), clamped to ``ChromeMetrics/columnMinWidth``…``columnMaxWidth``
/// and the room, then balanced (the narrowest width that keeps the line count, so the last line is not a stub).
struct TextColumn: Equatable {
    /// The text box, without padding.
    var size: CGSize
    var lines: Int
    var truncated: Bool

    static func fit(_ text: String, font: ChromeFont, widthRoom: CGFloat, heightRoom: CGFloat, maxLines: Int,
                    measurer: any TextMeasuring) -> TextColumn {
        let room = max(20, widthRoom)
        let lineCap = max(1, min(maxLines, Int((heightRoom + 0.5) / font.lineHeight)))
        let natural = measurer.width(of: text, font: font)
        func lineCount(_ width: CGFloat) -> Int {
            let size = measurer.size(of: text, font: font, maxWidth: width, maxLines: 10_000)
            return max(1, Int((size.height / font.lineHeight).rounded()))
        }
        if natural <= min(room, ChromeMetrics.columnShortWidth) {
            return TextColumn(size: CGSize(width: natural, height: font.lineHeight), lines: 1, truncated: false)
        }
        let target = (natural * font.lineHeight * ChromeMetrics.columnAspect).squareRoot()
        var width = min(room, min(max(target, ChromeMetrics.columnMinWidth), ChromeMetrics.columnMaxWidth))
        var lines = lineCount(width)
        // More lines than the room holds: widen toward the room before cutting anything.
        while lines > lineCap, width < room {
            width = min(room, width + 12)
            lines = lineCount(width)
        }
        if lines > 1, lines <= lineCap {
            var lo = max(20, width * 0.55), hi = width
            for _ in 0..<7 {
                let mid = (lo + hi) / 2
                if lineCount(mid) <= lines { hi = mid } else { lo = mid }
            }
            width = hi.rounded(.up)
        }
        let shown = min(lines, lineCap)
        let measured = measurer.size(of: text, font: font, maxWidth: width, maxLines: shown)
        return TextColumn(size: CGSize(width: min(width, measured.width), height: CGFloat(shown) * font.lineHeight),
                          lines: shown, truncated: lines > lineCap)
    }
}

/// Sizes and fits a row: `vertical` rows run top to bottom with upright items (the left/right arcs).
struct RowFitter {
    var vertical: Bool
    /// Room away from the row's line (depth) that items may use.
    var depthLimit: CGFloat
    /// A third of the row after the gaps (`maxElementAlongExtent`).
    var third: CGFloat
    var available: CGFloat
    var gap: CGFloat
    var measurer: any TextMeasuring

    func size(_ sizing: RowSizing, alongLimit: CGFloat, spanAlong: CGFloat? = nil) -> RowSize {
        func make(_ size: CGSize, lines: Int = 1, image: CGSize? = nil, truncated: Bool = false) -> RowSize {
            RowSize(size: size, along: vertical ? size.height : size.width, depth: vertical ? size.width : size.height,
                    lines: lines, imageSize: image, truncated: truncated)
        }
        switch sizing {
        case .fixed(let size):
            return make(size)
        case .image(let aspect, let maxSide, let extraHeight, let captionWidth):
            let aspect = aspect.isFinite && aspect > 0 ? min(max(aspect, 0.25), 4) : 1
            // Natural image box: fit maxSide × maxSide.
            var w = aspect >= 1 ? maxSide : maxSide * aspect
            var h = aspect >= 1 ? maxSide / aspect : maxSide
            // Depth budget (height for horizontal rows, width for vertical ones).
            if vertical {
                if w > depthLimit { h *= depthLimit / w; w = depthLimit }
            } else if h + extraHeight > depthLimit {
                let factor = max(0.1, (depthLimit - extraHeight) / h)
                w *= factor; h *= factor
            }
            var cardWidth = max(w, min(captionWidth, vertical ? depthLimit : alongLimit))
            var cardHeight = h + extraHeight
            let along = vertical ? cardHeight : cardWidth
            if along > alongLimit, along > 0 {
                if vertical {
                    let factor = max(0.05, (alongLimit - extraHeight) / h)
                    if factor < 1 { w *= factor; h *= factor }
                    cardWidth = max(w, min(captionWidth, depthLimit))
                } else {
                    if w > alongLimit {
                        let factor = alongLimit / w
                        w *= factor; h *= factor
                    }
                    cardWidth = alongLimit
                }
                cardHeight = h + extraHeight
            }
            // Round down so a row fitted to the arc never overflows it by rounding.
            let finalWidth = cardWidth.rounded(.down)
            return make(CGSize(width: finalWidth, height: cardHeight.rounded(.down)),
                        image: CGSize(width: w.rounded(.down), height: h.rounded(.down)),
                        truncated: captionWidth > finalWidth + 4)
        case .text(let text, let font, let maxLines):
            let padH = ChromeMetrics.pillPadding, padV = ChromeMetrics.pillVerticalPadding
            let widthLimit = (vertical ? depthLimit : alongLimit) - 2 * padH
            let heightLimit = (vertical ? alongLimit : depthLimit) - 2 * padV
            let column = TextColumn.fit(text, font: font, widthRoom: widthLimit, heightRoom: heightLimit, maxLines: maxLines,
                                        measurer: measurer)
            let height = max(ChromeMetrics.pillHeight, column.size.height + 2 * padV).rounded(.up)
            // +2: SwiftUI must wrap at the measured width, not a hair narrower.
            let width = min((column.size.width + 2 * padH + 2).rounded(.up),
                            max(2 * padH + 20, widthLimit + 2 * padH + 2).rounded(.down))
            return make(CGSize(width: width, height: height), lines: column.lines, truncated: column.truncated)
        case .singleLine(let text, let font, let height, let padding):
            let natural = (measurer.width(of: text, font: font) + 2 * padding).rounded(.up)
            let limit = max(vertical ? depthLimit : alongLimit, 2 * padding + 12).rounded(.down)
            return make(CGSize(width: min(natural, limit), height: height), truncated: natural > limit + 0.5)
        case .span(let minAlong, let depth):
            let along = max(minAlong, spanAlong ?? minAlong)
            return make(vertical ? CGSize(width: depth, height: along) : CGSize(width: along, height: depth))
        }
    }

    /// Sizes every request so the row fits `available` (flex, like InfoArc.place, but shrinking to fit).
    func fit(_ requests: [RowRequest]) -> [RowSize] {
        guard !requests.isEmpty else { return [] }
        let gaps = gap * CGFloat(requests.count - 1)
        var limits = requests.map { $0.capToThird ? third : available }
        var sizes = zip(requests, limits).map { size($0.sizing, alongLimit: $1) }
        for _ in 0..<4 {
            let fixedAlong = zip(requests, sizes).reduce(CGFloat(0)) { total, pair in
                pair.0.sizing.isShrinkable ? total : total + (pair.0.sizing.isSpan ? pair.0.sizing.minAlong : pair.1.along)
            }
            let shrinkable = zip(requests, sizes).filter { $0.0.sizing.isShrinkable }.reduce(CGFloat(0)) { $0 + $1.1.along }
            let room = available - gaps - fixedAlong
            guard shrinkable > room, shrinkable > 0 else { break }
            let factor = max(0.05, room / shrinkable)
            for index in requests.indices where requests[index].sizing.isShrinkable {
                limits[index] = max(requests[index].sizing.minAlong, sizes[index].along * factor)
                sizes[index] = size(requests[index].sizing, alongLimit: limits[index])
            }
        }
        // Spans take what is left.
        let spanCount = requests.filter { $0.sizing.isSpan }.count
        if spanCount > 0 {
            let used = zip(requests, sizes).reduce(CGFloat(0)) { $0 + ($1.0.sizing.isSpan ? 0 : $1.1.along) }
            let each = (available - gaps - used) / CGFloat(spanCount)
            for index in requests.indices where requests[index].sizing.isSpan {
                sizes[index] = size(requests[index].sizing, alongLimit: available, spanAlong: each)
            }
        }
        return sizes
    }
}

// MARK: - Content → row requests

extension ChromeContent {
    /// The row: badge first, then the controls, input, show elements or hint.
    /// `uncappedText`: text may take more than a third of the row (the left/right arcs, where a column runs down the
    /// arc, and compact mode's short, wide line). `omitField`: the typing field is a curved band, not a row item.
    func rowRequests(third: CGFloat, depthLimit: CGFloat, rowLength: CGFloat, preferredFieldWidth: CGFloat,
                     measurer: any TextMeasuring, scaleDepth: CGFloat, uncappedText: Bool = false, omitField: Bool = false)
        -> (requests: [RowRequest], trackRole: ChromeLayout.TrackRole?)
    {
        var requests: [RowRequest] = []
        var fieldWidth = preferredFieldWidth
        if let badge {
            // The badge never shrinks: a truncated "SIMULATI…" label defeats its purpose.
            let badgeWidth = min(third, (measurer.width(of: badge.text, font: .badge) + 18).rounded(.up))
            fieldWidth = min(fieldWidth, rowLength - badgeWidth - 2 * third * 0.05 - 8)
            requests.append(RowRequest(kind: .badge, sizing: .fixed(CGSize(width: badgeWidth, height: ChromeMetrics.badgeHeight)),
                                       capToThird: true, fullText: badge.text))
        }
        fieldWidth = max(min(ChromeMetrics.fieldMinWidth, preferredFieldWidth), fieldWidth.rounded(.down))
        var trackRole: ChromeLayout.TrackRole?
        switch (input, visibleControls) {
        case (.typing, _):
            if !omitField {
                requests.append(RowRequest(kind: .field, sizing: .fixed(CGSize(width: fieldWidth, height: ChromeMetrics.fieldHeight)),
                                           capToThird: false))
            }
        case (.listening, _), (.transcribing, _):
            requests.append(RowRequest(kind: .field, sizing: .span(minAlong: ChromeMetrics.minSpanAlong,
                                                                   depth: ChromeMetrics.waveformThickness), capToThird: false))
            trackRole = .waveform
        case (.none, .text?):
            if !omitField {
                requests.append(RowRequest(kind: .field, sizing: .fixed(CGSize(width: fieldWidth, height: ChromeMetrics.fieldHeight)),
                                           capToThird: false))
            }
        case (.none, .choice(let options, let multiple)?):
            let labelHeight = ChromeMetrics.captionHeight
            let hasImages = options.contains { $0.imageKey != nil }
            for (index, option) in options.enumerated() {
                if hasImages {
                    let captionWidth = measurer.width(of: option.label, font: .caption) + 2 * ChromeMetrics.pillPadding
                    requests.append(RowRequest(
                        kind: .option(index: index, imageSize: .zero),
                        sizing: .image(aspect: option.imageKey == nil ? 1 : option.aspect, maxSide: third,
                                       extraHeight: labelHeight + ChromeMetrics.captionGap, captionWidth: captionWidth),
                        capToThird: true, fullText: option.label))
                } else {
                    // Multiple choice adds the check circle (icon + spacing) inside tighter side padding.
                    let padding = multiple
                        ? ChromeMetrics.multipleOptionPadding + ChromeMetrics.multipleOptionIcon / 2
                        : ChromeMetrics.optionPadding
                    requests.append(RowRequest(kind: .option(index: index, imageSize: nil),
                                               sizing: .singleLine(option.label, .pill, height: ChromeMetrics.optionHeight,
                                                                   padding: padding),
                                               capToThird: true, fullText: option.label))
                }
            }
            if multiple {
                requests.append(RowRequest(kind: .confirm, sizing: .fixed(CGSize(width: ChromeMetrics.confirmSize,
                                                                                 height: ChromeMetrics.confirmSize)),
                                           capToThird: false))
            }
        case (.none, .scale(let isRange)?):
            requests.append(RowRequest(kind: .field, sizing: .span(minAlong: ChromeMetrics.minSpanAlong, depth: scaleDepth),
                                       capToThird: false))
            requests.append(RowRequest(kind: .confirm, sizing: .fixed(CGSize(width: ChromeMetrics.confirmSize,
                                                                             height: ChromeMetrics.confirmSize)),
                                       capToThird: false))
            trackRole = isRange ? .range : .slider
        case (.none, nil):
            for (index, element) in elements.enumerated() {
                switch element {
                case .text(let text):
                    // Long text runs narrow and tall (TextColumn); on the left/right arcs "tall" is along the arc.
                    requests.append(RowRequest(kind: .textPill(element: index, lines: 1),
                                               sizing: .text(text, .pill, maxLines: ChromeMetrics.pillMaxLines),
                                               capToThird: !uncappedText, fullText: text))
                case .image(_, let aspect, let caption):
                    let hasCaption = !(caption ?? "").isEmpty
                    let captionWidth = hasCaption
                        ? measurer.width(of: caption!, font: .caption) + 2 * ChromeMetrics.pillPadding : 0
                    requests.append(RowRequest(
                        kind: .imageCard(element: index, imageSize: .zero),
                        sizing: .image(aspect: aspect, maxSide: third,
                                       extraHeight: hasCaption ? ChromeMetrics.captionHeight + ChromeMetrics.captionGap : 0,
                                       captionWidth: captionWidth),
                        capToThird: true, fullText: hasCaption ? caption : nil))
                }
            }
            if elements.isEmpty, let hint = hintText {
                requests.append(RowRequest(kind: .hint, sizing: .singleLine(hint, .caption, height: ChromeMetrics.pillHeight,
                                                                            padding: ChromeMetrics.pillPadding),
                                           capToThird: false, fullText: hint))
            }
        }
        return (requests, trackRole)
    }

    /// The row item kind with its final measurements filled in.
    static func finalKind(_ kind: ChromeLayout.ItemKind, _ size: RowSize) -> ChromeLayout.ItemKind {
        switch kind {
        case .textPill(let element, _): .textPill(element: element, lines: size.lines)
        case .imageCard(let element, _): .imageCard(element: element, imageSize: size.imageSize ?? .zero)
        case .option(let index, let image): .option(index: index, imageSize: image == nil ? nil : size.imageSize)
        default: kind
        }
    }

    /// The text for the question band: a notice wins over the question.
    var bandText: (text: String, isNotice: Bool)? {
        if let notice, !notice.isEmpty { return (notice, true) }
        if let question, !question.isEmpty { return (question, false) }
        return nil
    }
}

/// Splits `text` into two lines at the word boundary nearest its middle.
func splitIntoTwoLines(_ text: String) -> (String, String) {
    let words = text.split(separator: " ", omittingEmptySubsequences: true).map(String.init)
    guard words.count > 1 else {
        let middle = text.index(text.startIndex, offsetBy: text.count / 2)
        return (String(text[..<middle]), String(text[middle...]))
    }
    var best = 1
    var bestDelta = Int.max
    for split in 1..<words.count {
        let first = words[..<split].joined(separator: " ").count
        let delta = abs(first - (text.count - first))
        if delta < bestDelta {
            bestDelta = delta
            best = split
        }
    }
    return (words[..<best].joined(separator: " "), words[best...].joined(separator: " "))
}

/// Cuts `text` with a trailing "…" until it is at most `width` wide.
func truncate(_ text: String, toWidth width: CGFloat, font: ChromeFont, measurer: any TextMeasuring) -> String {
    guard measurer.width(of: text, font: font) > width else { return text }
    var characters = Array(text)
    while !characters.isEmpty {
        characters.removeLast()
        let candidate = String(characters).trimmingCharacters(in: .whitespaces) + "…"
        if measurer.width(of: candidate, font: font) <= width { return candidate }
    }
    return "…"
}

// MARK: - Normal mode

struct ArcChromeBuilder {
    let layout: SlotLayout
    let arc: InfoArc
    let content: ChromeContent
    let measurer: any TextMeasuring

    var vertical: Bool { arc.elementAxisOffset != 0 }

    /// Typing (on any ask, or a message) and text asks show a field.
    var wantsField: Bool {
        if content.input == .typing { return true }
        if content.input == .none, case .text? = content.visibleControls { return true }
        return false
    }

    /// On the top/bottom and corner arcs the field is a curved band next to the question (ui-feedback.md #7).
    var curvedField: Bool { wantsField && !vertical }

    /// On the left/right arcs the (upright) question block is stacked right above the upright field.
    var stackedQuestion: Bool { wantsField && vertical && content.bandText != nil }

    /// Glyph tops point away from the arc's centre (bottom slots) or toward it (top slots). An upright glyph rotated
    /// to the tangent at the apex has its top at (sin θ, −cos θ).
    var upIsOutward: Bool {
        let theta = arc.elementRotation(atArcLength: 0)
        let normal = arc.outwardNormal(atArcLength: 0)
        return Double(normal.dx) * sin(theta) + Double(normal.dy) * -cos(theta) > 0
    }

    /// Depth of one question line band.
    func questionThickness(lines: Int, font: ChromeFont) -> CGFloat {
        CGFloat(lines) * font.lineHeight + 2 * ChromeMetrics.questionPadding
    }

    /// A multi-line text column on a top/bottom or corner arc: it tilts at most ``ChromeMetrics/columnMaxTilt``.
    private func isColumn(_ request: RowRequest, _ size: RowSize) -> Bool {
        guard !vertical, case .textPill = request.kind else { return false }
        return size.lines >= 2
    }

    private func clampedTilt(_ rotation: Double, limit: Double = ChromeMetrics.columnMaxTilt) -> Double {
        min(max(wrappedAngle(rotation), -limit), limit)
    }

    /// Half the extent of a rect of `size` rotated by `rotation` along the unit vector `normal`.
    private func halfExtent(_ size: CGSize, rotation: Double, along normal: CGVector) -> CGFloat {
        let c = CGFloat(cos(rotation)), s = CGFloat(sin(rotation))
        let u = abs(normal.dx * c + normal.dy * s), v = abs(-normal.dx * s + normal.dy * c)
        return size.width / 2 * u + size.height / 2 * v
    }

    /// Where a row item goes at arc length `s`: on the outer side of the baseline, rotated to the tangent (columns tilt
    /// less and are lifted until their nearest corner touches the tangent, so they grow away from the screen edge).
    private func frame(for request: RowRequest, size: RowSize, at s: CGFloat, followArc: Bool = false,
                       tiltLimit: Double = ChromeMetrics.columnMaxTilt) -> RotatedRect {
        var rotation = arc.elementRotation(atArcLength: s)
        var lift = size.depth / 2
        if isColumn(request, size), !followArc {
            rotation = clampedTilt(rotation, limit: tiltLimit)
            lift = halfExtent(size.size, rotation: rotation, along: arc.outwardNormal(atArcLength: s))
        }
        return RotatedRect(center: arc.point(atArcLength: s, offset: lift), size: size.size, rotation: rotation)
    }

    /// Keeps a text column inside the panel ("stays on screen" at all 8 positions): a column that would poke out
    /// (a less-tilted column near the end of a corner arc) tilts further toward the arc, step by step, and if even
    /// following the arc does not fit, loses lines (flagged `truncated`, revealed on hover and click).
    private func contained(_ request: RowRequest, size: RowSize, at s: CGFloat, tiltLimit: Double)
        -> (RowSize, RotatedRect)
    {
        var size = size
        var placed = frame(for: request, size: size, at: s, tiltLimit: tiltLimit)
        guard case .text(let text, let font, _) = request.sizing else { return (size, placed) }
        let panel = CGRect(origin: .zero, size: layout.panelSize).insetBy(dx: ChromeMetrics.panelInset, dy: ChromeMetrics.panelInset)
        var limit = tiltLimit
        while !panel.contains(placed.boundingBox), limit < .pi / 2 {
            limit += 5 * .pi / 180
            placed = frame(for: request, size: size, at: s, tiltLimit: limit)
        }
        if panel.contains(placed.boundingBox) { return (size, placed) }
        placed = frame(for: request, size: size, at: s, followArc: true)
        let padH = ChromeMetrics.pillPadding, padV = ChromeMetrics.pillVerticalPadding
        while !panel.contains(placed.boundingBox), size.lines > 1 {
            let lines = size.lines - 1
            let column = TextColumn.fit(text, font: font, widthRoom: size.size.width - 2 * padH - 2,
                                        heightRoom: CGFloat(lines) * font.lineHeight, maxLines: lines, measurer: measurer)
            let height = max(ChromeMetrics.pillHeight, column.size.height + 2 * padV).rounded(.up)
            let box = CGSize(width: size.size.width, height: height)
            size = RowSize(size: box, along: vertical ? box.height : box.width, depth: vertical ? box.width : box.height,
                           lines: column.lines, imageSize: nil, truncated: true)
            placed = frame(for: request, size: size, at: s, followArc: true)
        }
        return (size, placed)
    }

    func build() -> ChromeLayout {
        let gap = arc.gap
        let third = arc.maxElementAlongExtent
        let band = content.bandText
        // Keep room for the question band (two lines at the smallest size), and the curved field, outside the row.
        let minQuestionFont = ChromeFont(size: ChromeFont.questionMinSize, weight: .semibold)
        var questionReserve: CGFloat = 0
        if band != nil, !stackedQuestion {
            questionReserve = vertical ? 96 + gap : questionThickness(lines: 2, font: minQuestionFont) + gap
        }
        if curvedField { questionReserve += ChromeMetrics.fieldBandThickness + ChromeMetrics.fieldQuestionGap + gap }
        let depthLimit = max(ChromeMetrics.optionHeight, arc.bandDepth - questionReserve)
        let chord = 2 * arc.radius * CGFloat(sin(arc.halfSpan))
        let fieldWidth = vertical
            ? min(max(ChromeMetrics.fieldMinWidth, arc.bandDepth - 12), 260)
            : min(ChromeMetrics.fieldPreferredWidth, chord * 0.86)
        let scaleDepth = ChromeMetrics.thumbSize
        var (requests, trackRole) = content.rowRequests(
            third: third, depthLimit: depthLimit, rowLength: arc.length - gap, preferredFieldWidth: fieldWidth,
            measurer: measurer, scaleDepth: scaleDepth, uncappedText: vertical, omitField: curvedField)
        var block: QuestionBlock?
        if stackedQuestion, let band, let fieldIndex = requests.firstIndex(where: { $0.kind == .field }) {
            let made = questionBlock(text: band.text, isNotice: band.isNotice, width: fieldWidth)
            block = made
            requests.insert(RowRequest(kind: .hint, sizing: .fixed(made.size), capToThird: false, isQuestionBlock: true),
                            at: fieldIndex)
        }
        let fitter = RowFitter(vertical: vertical, depthLimit: depthLimit, third: third, available: arc.length, gap: gap,
                               measurer: measurer)
        let fitted = fitter.fit(requests)
        var sizes = fitted
        // A column tilted less than the arc takes more room along it. When the row then no longer fits the arc (a
        // crowded row on a corner arc), the columns tilt only as far toward the arc as the room requires.
        var tiltLimit = ChromeMetrics.columnMaxTilt
        let apexRotation = abs(wrappedAngle(arc.elementRotation(atArcLength: 0)))
        while true {
            sizes = fitted
            for index in sizes.indices where isColumn(requests[index], sizes[index]) {
                let phi = apexRotation - min(apexRotation, tiltLimit)
                let w = sizes[index].size.width, h = sizes[index].size.height
                sizes[index].along = w * CGFloat(abs(cos(phi))) + h * CGFloat(abs(sin(phi)))
            }
            let total = sizes.reduce(CGFloat(0), { $0 + $1.along }) + gap * CGFloat(max(0, sizes.count - 1))
            if total <= arc.length + 0.5 || tiltLimit >= apexRotation { break }
            tiltLimit = min(apexRotation, tiltLimit + 5 * .pi / 180)
        }
        if tiltLimit >= apexRotation { tiltLimit = .pi }  // follows the arc

        // Flex-centred placement on the baseline (InfoArc.place, with spans and shrinking).
        var items: [ChromeLayout.Item] = []
        var track: ChromeLayout.Track?
        var blockFrame: RotatedRect?
        let total = sizes.reduce(CGFloat(0)) { $0 + $1.along } + gap * CGFloat(max(0, sizes.count - 1))
        var cursor = -total / 2
        for (request, size) in zip(requests, sizes) {
            let s = cursor + size.along / 2
            if request.sizing.isSpan {
                let thickness = trackRole == .waveform ? ChromeMetrics.waveformThickness : ChromeMetrics.trackThickness
                track = .arc(arc, s0: cursor, s1: cursor + size.along, offset: size.depth / 2, thickness: thickness)
            } else if request.isQuestionBlock {
                blockFrame = frame(for: request, size: size, at: s)
            } else {
                let (finalSize, placed) = isColumn(request, size) || (vertical && request.kind.isTextPill)
                    ? contained(request, size: size, at: s, tiltLimit: tiltLimit)
                    : (size, frame(for: request, size: size, at: s))
                items.append(ChromeLayout.Item(kind: ChromeContent.finalKind(request.kind, finalSize), frame: placed,
                                               arcLength: s, truncated: finalSize.truncated,
                                               fullText: request.fullText))
            }
            cursor += size.along + gap
        }

        // How far out the row reaches: the question band (and the curved field) start beyond it.
        var rowOuter: CGFloat = 0
        for item in items {
            for corner in item.frame.corners { rowOuter = max(rowOuter, arc.radialDistance(of: corner) - arc.radius) }
        }
        if let track, case .arc(_, _, _, let offset, _) = track {
            let reach = trackRole == .waveform
                ? offset + ChromeMetrics.waveformThickness / 2
                : offset + ChromeMetrics.thumbSize / 2 + ChromeMetrics.valuePillGap + ChromeMetrics.captionHeight
            rowOuter = max(rowOuter, reach)
        }
        let start = rowOuter + gap * 0.8
        var question: ChromeLayout.Question?
        var field: ChromeLayout.CurvedField?
        if let block, let blockFrame {
            let line = ChromeLayout.QuestionLine(text: block.text, font: block.font, width: block.textWidth,
                                                 placement: .block(blockFrame, lines: block.lines))
            question = ChromeLayout.Question(lines: [line], background: .rect(blockFrame), isNotice: block.isNotice,
                                             truncated: block.truncated, fullText: block.text)
        } else if curvedField {
            let thickness = ChromeMetrics.fieldBandThickness
            if let band {
                if upIsOutward {
                    // Bottom-side arcs: the question reads first, so it is the outer band (higher on screen).
                    field = buildCurvedField(inner: start)
                    question = buildQuestion(text: band.text, isNotice: band.isNotice,
                                             inner: start + thickness + ChromeMetrics.fieldQuestionGap)
                } else {
                    // Top-side arcs: the inner band is higher on screen, so the question takes it.
                    let built = buildQuestion(text: band.text, isNotice: band.isNotice, inner: start)
                    question = built
                    field = buildCurvedField(inner: built.outerOffset(from: arc) + ChromeMetrics.fieldQuestionGap)
                }
            } else {
                field = buildCurvedField(inner: start)
            }
        } else if let band {
            question = buildQuestion(text: band.text, isNotice: band.isNotice, inner: start)
        }

        var buttons = ChromeLayout.cornerButtons(layout: layout, inwardAngle: arc.normalAngle)
        ChromeLayout.placeAccessories(&buttons, layout: layout, waiting: content.waiting,
                                      collapsed: content.askCollapsed && content.question != nil, measurer: measurer)
        var hits: [HitShape] = items.map { .rect($0.frame) }
        if let track {
            let extra = trackRole == .waveform ? 0 : (ChromeMetrics.thumbSize - ChromeMetrics.trackThickness) / 2 + 4
            hits.append(track.band(extra: extra))
        }
        switch question?.background {
        case .arc(let band)?: hits.append(.arc(band))
        case .rect(let rect)?: hits.append(.rect(rect))
        case nil: break
        }
        if let field { hits.append(.arc(field.band)) }
        hits.append(.circle(center: buttons.mic, radius: buttons.radius))
        hits.append(.circle(center: buttons.keyboard, radius: buttons.radius))
        hits.append(.circle(center: buttons.down, radius: buttons.downRadius))
        if let expand = buttons.expand { hits.append(.circle(center: expand, radius: buttons.downRadius)) }
        return ChromeLayout(mode: .normal, items: items, track: track, trackRole: trackRole, question: question,
                            buttons: buttons, stripRect: nil, visualRect: layout.visualRect, hitShapes: hits, field: field,
                            panelSize: layout.panelSize)
    }

    /// The curved typing field: a band `inner` points outside the baseline, centred on the arc's midpoint.
    func buildCurvedField(inner: CGFloat) -> ChromeLayout.CurvedField {
        let thickness = ChromeMetrics.fieldBandThickness
        let middle = arc.concentric(offset: inner + thickness / 2)
        let room = middle.length - thickness - 2 * ChromeMetrics.questionCapPadding
        let length = max(ChromeMetrics.fieldMinWidth, min(ChromeMetrics.fieldBandPreferredLength, room))
        let half = (length / 2).rounded(.down)
        let submit = ChromeMetrics.fieldSubmitRadius
        return ChromeLayout.CurvedField(
            arc: middle, band: middle.band(from: -half, to: half, inner: -thickness / 2, outer: thickness / 2),
            halfLength: half, thickness: thickness, upIsOutward: upIsOutward,
            iconS: -half + 2, textStart: -half + 15, textEnd: half - submit - 7, submitS: half - 1, submitRadius: submit)
    }

    struct QuestionBlock {
        var text: String
        var font: ChromeFont
        var size: CGSize
        var textWidth: CGFloat
        var lines: Int
        var truncated: Bool
        var isNotice: Bool
    }

    /// The upright question block stacked above the field on the left/right arcs, as wide as the field.
    func questionBlock(text: String, isNotice: Bool, width: CGFloat) -> QuestionBlock {
        var font = isNotice ? ChromeFont.notice : ChromeFont.question
        let maxLines = 3
        func lineCount(_ font: ChromeFont) -> Int {
            max(1, Int((measurer.size(of: text, font: font, maxWidth: width - 16, maxLines: 100).height / font.lineHeight)
                .rounded()))
        }
        while lineCount(font) > maxLines, font.size > ChromeFont.questionMinSize { font.size -= 0.5 }
        let fullLines = lineCount(font)
        let lines = min(maxLines, fullLines)
        let measured = measurer.size(of: text, font: font, maxWidth: width - 16, maxLines: lines)
        return QuestionBlock(
            text: text, font: font,
            size: CGSize(width: width, height: (CGFloat(lines) * font.lineHeight + 2 * ChromeMetrics.questionPadding).rounded(.up)),
            textWidth: measured.width, lines: lines, truncated: fullLines > lines, isNotice: isNotice)
    }

    /// The question (or notice) band `inner` points outside the baseline.
    func buildQuestion(text: String, isNotice: Bool, inner: CGFloat) -> ChromeLayout.Question {
        let baseFont = isNotice ? ChromeFont.notice : ChromeFont.question
        if vertical {
            // Upright block beside the column of controls, centred on the arc's apex.
            let width = max(90, min(220, arc.bandDepth - inner + SlotGeometry.panelPadding / 2))
            var font = baseFont
            var measured = measurer.size(of: text, font: font, maxWidth: width - 16, maxLines: 4)
            while measured.height > 3 * font.lineHeight + 0.5, font.size > ChromeFont.questionMinSize {
                font.size -= 0.5
                measured = measurer.size(of: text, font: font, maxWidth: width - 16, maxLines: 4)
            }
            let lines = max(1, Int((measured.height / font.lineHeight).rounded()))
            let fullLines = Int((measurer.size(of: text, font: font, maxWidth: width - 16, maxLines: 100).height
                / font.lineHeight).rounded())
            let size = CGSize(width: (measured.width + 16).rounded(.up), height: measured.height + 2 * ChromeMetrics.questionPadding)
            let center = arc.point(atArcLength: 0, offset: inner + size.width / 2)
            let rect = RotatedRect(center: center, size: size, rotation: 0)
            let line = ChromeLayout.QuestionLine(text: text, font: font, width: measured.width,
                                                 placement: .block(rect, lines: lines))
            return ChromeLayout.Question(lines: [line], background: .rect(rect), isNotice: isNotice,
                                         truncated: fullLines > lines, fullText: text)
        }

        // Curved: find the largest size that fits one line, else two lines, else truncate.
        let capPad = ChromeMetrics.questionCapPadding
        func available(atMidRadius r: CGFloat, thickness: CGFloat) -> CGFloat {
            r * CGFloat(2 * arc.halfSpan) - 2 * (thickness / 2 + capPad)
        }
        var chosen: (font: ChromeFont, lines: [String])?
        var truncated = false
        var size = baseFont.size
        while size >= ChromeFont.questionMinSize - 0.01 {
            let font = ChromeFont(size: size, weight: baseFont.weight)
            let thickness = questionThickness(lines: 1, font: font)
            if measurer.width(of: text, font: font) <= available(atMidRadius: arc.radius + inner + thickness / 2, thickness: thickness) {
                chosen = (font, [text])
                break
            }
            size -= 0.5
        }
        if chosen == nil {
            let font = ChromeFont(size: ChromeFont.questionMinSize, weight: baseFont.weight)
            let thickness = questionThickness(lines: 2, font: font)
            let room = available(atMidRadius: arc.radius + inner + thickness / 4, thickness: thickness)
            let (first, second) = splitIntoTwoLines(text)
            let cut = [truncate(first, toWidth: room, font: font, measurer: measurer),
                       truncate(second, toWidth: room, font: font, measurer: measurer)]
            truncated = cut[0] != first || cut[1] != second
            chosen = (font, cut)
        }
        let font = chosen!.font
        let lineTexts = chosen!.lines
        let thickness = questionThickness(lines: lineTexts.count, font: font)

        // Reading order runs top to bottom on screen: the first line is the outer one when "up" is outward.
        let upIsOutward = self.upIsOutward
        var lines: [ChromeLayout.QuestionLine] = []
        var maxWidth: CGFloat = 0
        for (index, lineText) in lineTexts.enumerated() {
            let slot = upIsOutward ? lineTexts.count - 1 - index : index
            let middle = inner + ChromeMetrics.questionPadding + (CGFloat(slot) + 0.5) * font.lineHeight
            let width = measurer.width(of: lineText, font: font)
            maxWidth = max(maxWidth, width)
            lines.append(ChromeLayout.QuestionLine(text: lineText, font: font, width: width,
                                                   placement: .arc(arc.concentric(offset: middle), upIsOutward: upIsOutward)))
        }
        let midRadius = arc.radius + inner + thickness / 2
        let halfAngle = Double((maxWidth / 2 + capPad) / midRadius)
        let band = ArcBand(center: arc.center, innerRadius: arc.radius + inner, outerRadius: arc.radius + inner + thickness,
                           startAngle: arc.normalAngle - arc.readingSign * halfAngle,
                           endAngle: arc.normalAngle + arc.readingSign * halfAngle)
        return ChromeLayout.Question(lines: lines, background: .arc(band), isNotice: isNotice, truncated: truncated,
                                     fullText: text)
    }
}

extension ChromeLayout.Question {
    /// How far outside `arc`'s baseline the band ends.
    func outerOffset(from arc: InfoArc) -> CGFloat {
        switch background {
        case .arc(let band): band.outerRadius - arc.radius
        case .rect(let rect): rect.corners.map { arc.radialDistance(of: $0) - arc.radius }.max() ?? 0
        }
    }
}

extension ChromeLayout.ItemKind {
    var isTextPill: Bool {
        if case .textPill = self { return true }
        return false
    }
}

// MARK: - Compact mode

struct StripChromeBuilder {
    let layout: SlotLayout
    let line: CompactLine
    let content: ChromeContent
    let measurer: any TextMeasuring

    func build() -> ChromeLayout {
        let height = line.contentHeight
        let cy = line.start.y
        let radius = ChromeMetrics.compactButtonRadius
        let buttonX = line.end.x - radius - 2
        let spacing = 2 * radius + 4
        let inward = layout.slot.side.inward
        let outward = atan2(-inward.dy, -inward.dx)
        var buttons = ChromeLayout.Buttons(
            mic: CGPoint(x: buttonX, y: cy - spacing), keyboard: CGPoint(x: buttonX, y: cy),
            down: CGPoint(x: buttonX, y: cy + spacing), radius: radius, downRadius: radius,
            downRotation: wrappedAngle(outward - .pi / 2))
        placeCompactAccessories(&buttons, spacing: spacing)
        let x0 = line.start.x
        let x1 = buttonX - radius - line.gap
        let width = max(60, x1 - x0)
        let gap = line.gap
        let band = content.bandText
        let rowY = band == nil ? cy : cy + 12
        let depthLimit = band == nil ? height - 12 : height - 40
        let third = max(0, (width - 2 * gap) / 3)
        let (requests, trackRole) = content.rowRequests(
            third: third, depthLimit: depthLimit, rowLength: width - gap, preferredFieldWidth: width, measurer: measurer,
            scaleDepth: ChromeMetrics.thumbSize, uncappedText: true)
        let fitter = RowFitter(vertical: false, depthLimit: depthLimit, third: third, available: width, gap: gap,
                               measurer: measurer)
        let sizes = fitter.fit(requests)
        let total = sizes.reduce(CGFloat(0)) { $0 + $1.along } + gap * CGFloat(max(0, sizes.count - 1))
        // Content reads from the small visual rightward (a spanning track or waveform still fills the width).
        let mid = x0 + ChromeMetrics.compactInset + total / 2
        var cursor = -total / 2
        var items: [ChromeLayout.Item] = []
        var track: ChromeLayout.Track?
        for (request, size) in zip(requests, sizes) {
            let s = cursor + size.along / 2
            if request.sizing.isSpan {
                let thickness = trackRole == .waveform ? ChromeMetrics.waveformThickness : ChromeMetrics.trackThickness
                track = .line(start: CGPoint(x: mid + cursor, y: rowY), end: CGPoint(x: mid + cursor + size.along, y: rowY),
                              thickness: thickness)
            } else {
                let frame = RotatedRect(center: CGPoint(x: mid + s, y: rowY), size: size.size, rotation: 0)
                items.append(ChromeLayout.Item(kind: ChromeContent.finalKind(request.kind, size), frame: frame, arcLength: s,
                                               truncated: size.truncated, fullText: request.fullText))
            }
            cursor += size.along + gap
        }

        var question: ChromeLayout.Question?
        if let band {
            var font = band.isNotice ? ChromeFont.notice : ChromeFont(size: 12.5, weight: .semibold)
            while measurer.width(of: band.text, font: font) > width, font.size > ChromeFont.questionMinSize {
                font.size -= 0.5
            }
            let text = truncate(band.text, toWidth: width, font: font, measurer: measurer)
            let textWidth = measurer.width(of: text, font: font)
            let rect = RotatedRect(center: CGPoint(x: x0 + ChromeMetrics.compactInset + (textWidth + 16) / 2,
                                                   y: cy - height / 2 + 8 + font.lineHeight / 2),
                                   size: CGSize(width: textWidth + 16, height: font.lineHeight + 6), rotation: 0)
            question = ChromeLayout.Question(
                lines: [ChromeLayout.QuestionLine(text: text, font: font, width: textWidth, placement: .block(rect, lines: 1))],
                background: .rect(rect), isNotice: band.isNotice, truncated: text != band.text, fullText: band.text)
        }

        // The region the line is laid out in, right of the small visual. It is not drawn (ui-feedback.md #8: no
        // background), so only the elements themselves catch the pointer.
        let stripX = line.start.x - ChromeMetrics.compactInset / 2
        let strip = CGRect(x: stripX, y: cy - height / 2, width: line.end.x + 6 - stripX, height: height)
        var hits: [HitShape] = items.map { .rect($0.frame) }
        if let track {
            let extra = trackRole == .waveform ? 0 : (ChromeMetrics.thumbSize - ChromeMetrics.trackThickness) / 2 + 4
            hits.append(track.band(extra: extra))
        }
        if case .rect(let rect)? = question?.background { hits.append(.rect(rect)) }
        for point in [buttons.mic, buttons.keyboard, buttons.down] { hits.append(.circle(center: point, radius: radius)) }
        if let expand = buttons.expand { hits.append(.circle(center: expand, radius: radius)) }
        return ChromeLayout(mode: .compact, items: items, track: track, trackRole: trackRole, question: question,
                            buttons: buttons, stripRect: strip, visualRect: layout.visualRect, hitShapes: hits,
                            panelSize: layout.panelSize)
    }

    /// Compact mode: `^` one button-spacing left of the down-arrow. The badge rides the down-arrow's rim, as close to its
    /// top-trailing corner as the tight button column allows (the buttons are 4 pt apart and the panel ends 14 pt past
    /// the column on the right-hand slots): the first of top-trailing, trailing, bottom-trailing and bottom where it stays
    /// inside the panel, clear of the mic, keyboard and `^`, off the chevron (≥ 0.4 r from the down-arrow's centre) and
    /// still on the rim; the least bad of them otherwise.
    func placeCompactAccessories(_ buttons: inout ChromeLayout.Buttons, spacing: CGFloat) {
        let r = buttons.downRadius
        if content.askCollapsed, content.question != nil {
            buttons.expand = CGPoint(x: buttons.down.x - spacing, y: buttons.down.y)
        }
        guard content.waiting > 0 else { return }
        let size = ChromeLayout.waitingBadgeSize(content.waiting, downRadius: r, measurer: measurer)
        let panel = CGRect(origin: .zero, size: layout.panelSize).insetBy(dx: 1, dy: 1)
        let down = buttons.down
        func distance(_ rect: CGRect, _ point: CGPoint) -> CGFloat {
            let nearest = CGPoint(x: min(max(point.x, rect.minX), rect.maxX), y: min(max(point.y, rect.minY), rect.maxY))
            return hypot(nearest.x - point.x, nearest.y - point.y)
        }
        var others: [(CGPoint, CGFloat)] = [(buttons.mic, buttons.radius), (buttons.keyboard, buttons.radius)]
        if let expand = buttons.expand { others.append((expand, r)) }
        var best: (score: CGFloat, rect: CGRect)?
        // y-down: −45° is up and trailing.
        for degrees in [-45.0, 0, 45, 90] {
            let angle = degrees * .pi / 180
            var step: CGFloat = -2
            while step <= ChromeMetrics.accessoryMaxPush {
                let d = r + step
                let center = CGPoint(x: down.x + d * CGFloat(cos(angle)), y: down.y + d * CGFloat(sin(angle)))
                let rect = ChromeOverlay.clamp(CGRect(x: center.x - size.width / 2, y: center.y - size.height / 2,
                                                      width: size.width, height: size.height), into: panel)
                let clearOfButtons = others.allSatisfy { distance(rect, $0.0) >= $0.1 + ChromeMetrics.accessoryClearance }
                let offChevron = distance(rect, down)
                if clearOfButtons, offChevron >= 0.4 * r, offChevron < r {
                    buttons.badge = CGPoint(x: rect.midX, y: rect.midY)
                    buttons.badgeSize = size
                    return
                }
                let score = (clearOfButtons ? 100 : 0) + min(offChevron, r)
                if best == nil || score > best!.score { best = (score, rect) }
                step += ChromeMetrics.accessoryPushStep
            }
        }
        if let best {
            buttons.badge = CGPoint(x: best.rect.midX, y: best.rect.midY)
            buttons.badgeSize = size
        }
    }
}
