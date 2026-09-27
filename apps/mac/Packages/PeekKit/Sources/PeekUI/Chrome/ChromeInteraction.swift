import CoreGraphics
import Foundation
import PeekCore

// What the pointer is over, what is cut short, and where the reveal overlays go (ui-feedback.md #1, #2, #5).
// Pure, like ChromeLayout: the panel feeds the pointer in from its frame tick (hover must work while Peek is
// inactive and its panel is not key, where tracking areas and `.onHover` do not fire), and the views draw.

/// A piece of chrome the pointer can be over.
public enum ChromeTarget: Hashable, Sendable {
    /// A row item, by ``ChromeLayout/Item/identity`` ("option-1", "text-0", "image-0", "confirm", "field", "badge", "hint").
    case item(String)
    case question
    /// The curved typing field's band.
    case field
    /// The curved typing field's send button.
    case submit
    /// The slider/range track and its thumbs.
    case track
    case mic
    case keyboard
    case down
    /// The `^` that expands a compact ask (peek 0.1.2).
    case expand
    /// The open tap-to-expand popup.
    case popup
}

extension ChromeLayout.Item {
    /// A stable identity per row position and role (SwiftUI animations, hover targets).
    public var identity: String {
        switch kind {
        case .badge: "badge"
        case .textPill(let element, _): "text-\(element)"
        case .imageCard(let element, _): "image-\(element)"
        case .option(let index, _): "option-\(index)"
        case .field: "field"
        case .confirm: "confirm"
        case .hint: "hint"
        }
    }
}

extension ChromeLayout {
    public func item(identity: String) -> Item? { items.first { $0.identity == identity } }

    /// The topmost piece of chrome under `point` (panel-local), not counting the drawing or overlays.
    public func target(at point: CGPoint) -> ChromeTarget? {
        if hypot(point.x - buttons.down.x, point.y - buttons.down.y) <= buttons.downRadius + 3 { return .down }
        if let expand = buttons.expand, hypot(point.x - expand.x, point.y - expand.y) <= buttons.downRadius + 2 {
            return .expand
        }
        if hypot(point.x - buttons.mic.x, point.y - buttons.mic.y) <= buttons.radius + 2 { return .mic }
        if hypot(point.x - buttons.keyboard.x, point.y - buttons.keyboard.y) <= buttons.radius + 2 { return .keyboard }
        if let field {
            let submit = field.submitCenter
            if hypot(point.x - submit.x, point.y - submit.y) <= field.submitRadius + 3 { return .submit }
            if field.band.contains(point, slop: 2) { return .field }
        }
        for item in items.reversed() where item.frame.contains(point, slop: 2) { return .item(item.identity) }
        if let track, trackRole != .waveform {
            let extra = (ChromeMetrics.thumbSize - ChromeMetrics.trackThickness) / 2 + 4
            if track.band(extra: extra).contains(point) { return .track }
        }
        switch question?.background {
        case .arc(let band)? where band.contains(point, slop: 2): return .question
        case .rect(let rect)? where rect.contains(point, slop: 2): return .question
        default: return nil
        }
    }

    /// The complete text of `target` when what is shown is cut short with "…"; nil when it is all visible.
    public func hiddenText(of target: ChromeTarget) -> String? {
        switch target {
        case .item(let identity):
            guard let item = item(identity: identity), item.truncated, let text = item.fullText, !text.isEmpty else { return nil }
            return text
        case .question:
            guard let question, question.truncated, !question.fullText.isEmpty else { return nil }
            return question.fullText
        default:
            return nil
        }
    }

    /// Static text that is cut short: a click opens it in the popup (ui-feedback.md #5). Options are not
    /// expandable (a click answers); their labels are revealed on hover only.
    public func isExpandable(_ target: ChromeTarget) -> Bool {
        guard hiddenText(of: target) != nil else { return false }
        switch target {
        case .question: return true
        case .item(let identity): return identity.hasPrefix("text-") || identity.hasPrefix("image-")
        default: return false
        }
    }

    /// The upright box a target occupies (panel-local).
    public func anchorBox(of target: ChromeTarget) -> CGRect? {
        switch target {
        case .item(let identity): return item(identity: identity)?.frame.boundingBox
        case .question:
            switch question?.background {
            case .arc(let band)?: return band.boundingBox
            case .rect(let rect)?: return rect.boundingBox
            case nil: return nil
            }
        case .field: return field?.band.boundingBox
        case .submit:
            guard let field else { return nil }
            let c = field.submitCenter, r = field.submitRadius
            return CGRect(x: c.x - r, y: c.y - r, width: 2 * r, height: 2 * r)
        case .track: return track?.boundingBox(thickness: ChromeMetrics.thumbSize)
        case .mic: return Self.circleBox(buttons.mic, buttons.radius)
        case .keyboard: return Self.circleBox(buttons.keyboard, buttons.radius)
        case .down: return Self.circleBox(buttons.down, buttons.downRadius)
        case .expand: return buttons.expand.map { Self.circleBox($0, buttons.downRadius) }
        case .popup: return nil
        }
    }

    static func circleBox(_ center: CGPoint, _ radius: CGFloat) -> CGRect {
        CGRect(x: center.x - radius, y: center.y - radius, width: 2 * radius, height: 2 * radius)
    }
}

/// A glass overlay with the complete text of something cut short: the hover tooltip (#1) or the tap-to-expand
/// popup (#5). Always upright and inside the panel.
public struct ChromeOverlay: Sendable, Equatable {
    public enum Style: Sendable, Equatable {
        /// Beside the element, while it is hovered. Never takes the pointer.
        case tooltip
        /// Over the element (in place), after a click; a click, Esc or a click outside closes it.
        case popup
    }

    public var style: Style
    public var target: ChromeTarget
    public var text: String
    public var font: ChromeFont
    /// Panel-local, upright.
    public var frame: CGRect
    /// The text is taller than the panel allows: the popup scrolls.
    public var scrolls: Bool
    /// Where the overlay grows from (panel-local): the element's centre, for the bounce.
    public var origin: CGPoint

    public static let tooltipFont = ChromeFont(size: 12, weight: .medium)
    public static let popupFont = ChromeFont(size: 13, weight: .medium)

    public static func make(_ style: Style, for target: ChromeTarget, text: String, layout: ChromeLayout,
                            measurer: any TextMeasuring) -> ChromeOverlay? {
        guard let box = layout.anchorBox(of: target), layout.panelSize.width > 0 else { return nil }
        let font = style == .tooltip ? tooltipFont : popupFont
        let pad = ChromeMetrics.overlayPadding
        let panel = CGRect(origin: .zero, size: layout.panelSize).insetBy(dx: ChromeMetrics.panelInset, dy: ChromeMetrics.panelInset)
        let maxWidth = min(style == .tooltip ? ChromeMetrics.tooltipMaxWidth : ChromeMetrics.popupMaxWidth, panel.width)
        let measured = measurer.size(of: text, font: font, maxWidth: maxWidth - 2 * pad, maxLines: 1000)
        let fullHeight = measured.height + 2 * pad * (style == .tooltip ? 0.7 : 1)
        let height = min(fullHeight, panel.height)
        let size = CGSize(width: (measured.width + 2 * pad + 2).rounded(.up), height: height.rounded(.up))
        let center = CGPoint(x: box.midX, y: box.midY)
        let frame: CGRect
        switch style {
        case .popup:
            frame = clamp(CGRect(x: center.x - size.width / 2, y: center.y - size.height / 2, width: size.width,
                                 height: size.height), into: panel)
        case .tooltip:
            let away = CGVector(dx: center.x - layout.visualRect.midX, dy: center.y - layout.visualRect.midY)
            let visual = layout.visualRect
            frame = besideBox(size: size, box: box, away: away, panel: panel) { point in
                layout.isOverChrome(point) || visual.contains(point)
            }
        }
        return ChromeOverlay(style: style, target: target, text: text, font: font, frame: frame,
                             scrolls: fullHeight > panel.height + 0.5, origin: center)
    }

    /// Next to `box`, inside `panel`: the side that covers the least other chrome (`occupied`), preferring the side
    /// facing away from the visual (toward the screen centre) among equals.
    static func besideBox(size: CGSize, box: CGRect, away: CGVector, panel: CGRect,
                          occupied: (CGPoint) -> Bool = { _ in false }) -> CGRect {
        let gap: CGFloat = 6
        let length = max(hypot(away.dx, away.dy), 0.001)
        let d = CGVector(dx: away.dx / length, dy: away.dy / length)
        let candidates: [(CGVector, CGRect)] = [
            (CGVector(dx: 0, dy: 1), CGRect(x: box.midX - size.width / 2, y: box.maxY + gap, width: size.width, height: size.height)),
            (CGVector(dx: 0, dy: -1), CGRect(x: box.midX - size.width / 2, y: box.minY - gap - size.height, width: size.width,
                                             height: size.height)),
            (CGVector(dx: 1, dy: 0), CGRect(x: box.maxX + gap, y: box.midY - size.height / 2, width: size.width, height: size.height)),
            (CGVector(dx: -1, dy: 0), CGRect(x: box.minX - gap - size.width, y: box.midY - size.height / 2, width: size.width,
                                             height: size.height)),
        ]
        func covered(_ rect: CGRect) -> Double {
            var hits = 0, total = 0
            for i in 0..<7 {
                for j in 0..<3 {
                    let point = CGPoint(x: rect.minX + rect.width * (CGFloat(i) + 0.5) / 7,
                                        y: rect.minY + rect.height * (CGFloat(j) + 0.5) / 3)
                    total += 1
                    if occupied(point) { hits += 1 }
                }
            }
            return Double(hits) / Double(total)
        }
        var best: (score: Double, rect: CGRect)?
        for (direction, rect) in candidates {
            let placed = clamp(rect, into: panel)
            // Clamping that slides it over the element itself defeats the point.
            if placed.intersects(box.insetBy(dx: 3, dy: 3)) { continue }
            let facing = Double(direction.dx * d.dx + direction.dy * d.dy)
            let score = facing * 0.6 - covered(placed) * 3 - (placed == rect ? 0 : 0.2)
            if best == nil || score > best!.score { best = (score, placed) }
        }
        return best?.rect ?? clamp(candidates[0].1, into: panel)
    }

    static func clamp(_ rect: CGRect, into panel: CGRect) -> CGRect {
        var r = rect
        r.size.width = min(r.width, panel.width)
        r.size.height = min(r.height, panel.height)
        r.origin.x = min(max(r.minX, panel.minX), panel.maxX - r.width)
        r.origin.y = min(max(r.minY, panel.minY), panel.maxY - r.height)
        return r
    }
}
