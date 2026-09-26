import CoreGraphics
import Foundation

// Slot geometry: pure functions from a screen's visibleFrame, a slot and a mode
// to the panel frame and everything laid out inside it (BLUEPRINT §8.5, B2).
//
// Coordinate systems
//   * "screen" rects and points are AppKit global coordinates: y up, origin at
//     the bottom-left of the primary screen (what NSScreen.visibleFrame and
//     NSWindow.frame use).
//   * "panel-local" points are y-down with the origin at the panel's top-left,
//     in points: the coordinate system of SwiftUI and of a flipped NSView.
//   * "units" are the drawing's 100 × 100 square, y-down (visual.md A2).
//
// Proportions measured from understanding/peek all-positions.jpg (1280 × 832):
//   visual circle Ø ≈ 150 px (11.7% of the width), edge gap ≈ 0.45 r;
//   the information arc is NOT concentric with the circle: its baseline apex
//   is ≈ 1.3 r from the circle centre toward the screen centre, its radius is
//   ≈ 4.5 r (≈ 2.25 × the diameter) and it spans ≈ ±32°. Its endpoints then lie
//   ≈ 2.45 r from the circle centre, which is the "2.3–2.6 × r" in the notes.
//   Arcs face the screen centre in compass steps (the corner arcs are exactly
//   diagonal in the image), so arcs never run off the screen edges.

public enum SlotGeometry {
    // Normal mode.
    public static let visualDiameterFraction: CGFloat = 0.12
    public static let visualDiameterRange: ClosedRange<CGFloat> = 88...240
    public static let edgeMarginRatio: CGFloat = 0.45
    public static let arcApexRatio: CGFloat = 1.3
    public static let arcRadiusRatio: CGFloat = 4.5
    public static let arcHalfSpan: Double = 32 * .pi / 180
    /// Gap between elements on the arc, as a fraction of the visual radius.
    public static let elementGapRatio: CGFloat = 0.11
    /// A caption or label pill under an image.
    public static let pillHeight: CGFloat = 22
    /// The curved question text above the answer controls, plus its gap.
    public static let questionBand: CGFloat = 34
    /// Room around content for shadows and the slide spring's overshoot.
    public static let panelPadding: CGFloat = 24

    // Compact mode (understanding.md "Settings": a line instead of the arc, a small visual on the left).
    public static let compactVisualFraction: CGFloat = 0.03
    public static let compactVisualRange: ClosedRange<CGFloat> = 32...48
    public static let compactLineFraction: CGFloat = 0.25
    public static let compactLineRange: ClosedRange<CGFloat> = 300...460
    /// Height of the compact content row (question line + controls).
    public static let compactContentHeight: CGFloat = 88
    public static let compactEdgeMargin: CGFloat = 12
    public static let compactGap: CGFloat = 10

    /// The layout of one slot on a screen. `visibleFrame` is `NSScreen.visibleFrame`.
    public static func layout(slot: SlotIndex, mode: DisplayMode, visibleFrame: CGRect) -> SlotLayout {
        switch mode {
        case .normal: normalLayout(slot: slot, visibleFrame: visibleFrame)
        case .compact: compactLayout(slot: slot, visibleFrame: visibleFrame)
        }
    }

    /// All 8 layouts for a screen.
    public static func layouts(mode: DisplayMode, visibleFrame: CGRect) -> [SlotIndex: SlotLayout] {
        Dictionary(uniqueKeysWithValues: SlotIndex.allCases.map { ($0, layout(slot: $0, mode: mode, visibleFrame: visibleFrame)) })
    }

    /// Visual circle diameter in normal mode: about 12% of the screen width, clamped.
    public static func visualDiameter(screenWidth: CGFloat) -> CGFloat {
        clamp(screenWidth * visualDiameterFraction, visualDiameterRange)
    }

    // MARK: Normal

    private static func normalLayout(slot: SlotIndex, visibleFrame vf: CGRect) -> SlotLayout {
        let width = max(vf.width, 1)
        let height = max(vf.height, 1)
        let r = visualDiameter(screenWidth: width) / 2
        let side = slot.side
        let inward = CGVector(dx: side.inward.dx, dy: side.inward.dy)
        let margin = r * edgeMarginRatio

        // Screen-local, y-down, origin at the visible frame's top-left.
        let center = CGPoint(
            x: anchorX(side, width: width, inset: margin + r),
            y: anchorY(side, height: height, inset: margin + r))
        let arcRadius = r * arcRadiusRatio
        let arcCenter = CGPoint(
            x: center.x - inward.dx * (arcRadius - r * arcApexRatio),
            y: center.y - inward.dy * (arcRadius - r * arcApexRatio))
        let normalAngle = atan2(Double(inward.dy), Double(inward.dx))
        let gap = r * elementGapRatio
        let length = arcRadius * CGFloat(2 * arcHalfSpan)
        let maxAlong = (length - 2 * gap) / 3
        // Largest state: an ask whose options carry square images (maxAlong) with label pills,
        // topped by the question arc. A show (image + caption) is shallower.
        let bandDepth = maxAlong + gap / 2 + pillHeight + questionBand

        var content = CGRect(x: center.x - r, y: center.y - r, width: 2 * r, height: 2 * r)
        content = content.union(
            sectorBounds(center: arcCenter, inner: arcRadius - 2, outer: arcRadius + bandDepth,
                         from: normalAngle - arcHalfSpan, to: normalAngle + arcHalfSpan))
        content = content.insetBy(dx: -panelPadding, dy: -panelPadding)

        var panelLocal = extendToOutwardEdges(content, side: side, width: width, height: height)
        panelLocal = panelLocal.intersection(CGRect(x: 0, y: 0, width: width, height: height)).integralRect
        let origin = panelLocal.origin

        let arc = InfoArc(
            center: arcCenter - origin, radius: arcRadius, normalAngle: normalAngle, halfSpan: arcHalfSpan,
            bandDepth: bandDepth, gap: gap, readingSign: readingSign(normalAngle),
            elementAxisOffset: side.isVertical ? .pi / 2 : 0)
        let visualRect = CGRect(x: center.x - r - origin.x, y: center.y - r - origin.y, width: 2 * r, height: 2 * r)
        let contentLocal = content.offsetBy(dx: -origin.x, dy: -origin.y)
            .intersection(CGRect(origin: .zero, size: panelLocal.size))

        return SlotLayout(
            slot: slot, mode: .normal, visibleFrame: vf,
            panelFrame: toScreen(panelLocal, visibleFrame: vf),
            visualRect: visualRect,
            facing: atan2(Double(height / 2 - center.y), Double(width / 2 - center.x)),
            arc: arc, line: nil, contentBounds: contentLocal,
            hiddenOffset: hiddenOffset(content: contentLocal, panelSize: panelLocal.size, inward: inward))
    }

    // MARK: Compact

    private static func compactLayout(slot: SlotIndex, visibleFrame vf: CGRect) -> SlotLayout {
        let width = max(vf.width, 1)
        let height = max(vf.height, 1)
        let side = slot.side
        let inward = CGVector(dx: side.inward.dx, dy: side.inward.dy)
        let diameter = clamp(width * compactVisualFraction, compactVisualRange)
        let lineLength = clamp(width * compactLineFraction, compactLineRange)
        let rowHeight = max(diameter, compactContentHeight)
        let rowWidth = diameter + compactGap + lineLength

        let rowOrigin = CGPoint(
            x: anchorX(side, width: width, inset: compactEdgeMargin + rowWidth / 2) - rowWidth / 2,
            y: anchorY(side, height: height, inset: compactEdgeMargin + rowHeight / 2) - rowHeight / 2)
        let row = CGRect(origin: rowOrigin, size: CGSize(width: rowWidth, height: rowHeight))
        let visualCenter = CGPoint(x: row.minX + diameter / 2, y: row.midY)
        let content = row.insetBy(dx: -panelPadding, dy: -panelPadding)

        var panelLocal = extendToOutwardEdges(content, side: side, width: width, height: height)
        panelLocal = panelLocal.intersection(CGRect(x: 0, y: 0, width: width, height: height)).integralRect
        let origin = panelLocal.origin

        let lineStart = CGPoint(x: row.minX + diameter + compactGap, y: row.midY) - origin
        let lineEnd = CGPoint(x: row.maxX, y: row.midY) - origin
        let line = CompactLine(start: lineStart, end: lineEnd, contentHeight: rowHeight, gap: compactGap)
        let visualRect = CGRect(
            x: visualCenter.x - diameter / 2 - origin.x, y: visualCenter.y - diameter / 2 - origin.y,
            width: diameter, height: diameter)
        let contentLocal = content.offsetBy(dx: -origin.x, dy: -origin.y)
            .intersection(CGRect(origin: .zero, size: panelLocal.size))

        return SlotLayout(
            slot: slot, mode: .compact, visibleFrame: vf,
            panelFrame: toScreen(panelLocal, visibleFrame: vf),
            visualRect: visualRect,
            facing: atan2(Double(height / 2 - visualCenter.y), Double(width / 2 - visualCenter.x)),
            arc: nil, line: line, contentBounds: contentLocal,
            hiddenOffset: hiddenOffset(content: contentLocal, panelSize: panelLocal.size, inward: inward))
    }

    // MARK: Helpers

    /// X of a slot's anchor point: `inset` from the left/right edge, or the horizontal centre.
    private static func anchorX(_ side: SlotSide, width: CGFloat, inset: CGFloat) -> CGFloat {
        switch side {
        case .topLeft, .left, .bottomLeft: inset
        case .topRight, .right, .bottomRight: width - inset
        case .top, .bottom: width / 2
        }
    }

    /// Y (y-down) of a slot's anchor point: `inset` from the top/bottom edge, or the vertical centre.
    private static func anchorY(_ side: SlotSide, height: CGFloat, inset: CGFloat) -> CGFloat {
        switch side {
        case .topLeft, .top, .topRight: inset
        case .bottomLeft, .bottom, .bottomRight: height - inset
        case .left, .right: height / 2
        }
    }

    /// Extends a rect to the visible-frame edges the slot slides out of, so the
    /// slide starts exactly at the edge (the window clips the content).
    private static func extendToOutwardEdges(_ rect: CGRect, side: SlotSide, width: CGFloat, height: CGFloat) -> CGRect {
        var minX = rect.minX, maxX = rect.maxX, minY = rect.minY, maxY = rect.maxY
        let outward = (dx: -side.inward.dx, dy: -side.inward.dy)
        if outward.dx < -0.01 { minX = 0 }
        if outward.dx > 0.01 { maxX = width }
        if outward.dy < -0.01 { minY = 0 }
        if outward.dy > 0.01 { maxY = height }
        return CGRect(x: minX, y: minY, width: maxX - minX, height: maxY - minY)
    }

    /// The translation (panel-local, y-down) that moves `content` fully outside the
    /// panel through the outward edge: the hidden end of the slide animation.
    static func hiddenOffset(content: CGRect, panelSize: CGSize, inward: CGVector) -> CGVector {
        let outward = CGVector(dx: -inward.dx, dy: -inward.dy)
        var travel = CGFloat.infinity
        if outward.dx < -0.01 { travel = min(travel, content.maxX / -outward.dx) }
        if outward.dx > 0.01 { travel = min(travel, (panelSize.width - content.minX) / outward.dx) }
        if outward.dy < -0.01 { travel = min(travel, content.maxY / -outward.dy) }
        if outward.dy > 0.01 { travel = min(travel, (panelSize.height - content.minY) / outward.dy) }
        guard travel.isFinite else { return .zero }
        travel = travel.rounded(.up) + 1
        return CGVector(dx: outward.dx * travel, dy: outward.dy * travel)
    }

    /// Bounding box of an annular sector (y-down angles, `from` < `to`).
    static func sectorBounds(center: CGPoint, inner: CGFloat, outer: CGFloat, from: Double, to: Double) -> CGRect {
        var angles = [from, to]
        var k = (from / (.pi / 2)).rounded(.up)
        while k * (.pi / 2) <= to {
            angles.append(k * (.pi / 2))
            k += 1
        }
        var points: [CGPoint] = []
        for angle in angles {
            for radius in [inner, outer] {
                points.append(CGPoint(x: center.x + radius * CGFloat(cos(angle)), y: center.y + radius * CGFloat(sin(angle))))
            }
        }
        let xs = points.map(\.x), ys = points.map(\.y)
        return CGRect(x: xs.min()!, y: ys.min()!, width: xs.max()! - xs.min()!, height: ys.max()! - ys.min()!)
    }

    /// +1 when reading order (left→right, or top→bottom on vertical arcs) runs toward increasing angles.
    static func readingSign(_ normalAngle: Double) -> Double {
        let tangentX = -sin(normalAngle)
        if tangentX > 1e-6 { return 1 }
        if tangentX < -1e-6 { return -1 }
        return cos(normalAngle) > 0 ? 1 : -1
    }

    /// Screen-local y-down rect → AppKit global rect.
    private static func toScreen(_ rect: CGRect, visibleFrame vf: CGRect) -> CGRect {
        CGRect(x: vf.minX + rect.minX, y: vf.maxY - rect.maxY, width: rect.width, height: rect.height)
    }

    static func clamp(_ value: CGFloat, _ range: ClosedRange<CGFloat>) -> CGFloat {
        min(max(value, range.lowerBound), range.upperBound)
    }
}

// MARK: - Layout results

/// Everything about one slot's panel. See the coordinate notes at the top of this file.
public struct SlotLayout: Sendable, Equatable {
    public let slot: SlotIndex
    public let mode: DisplayMode
    /// The `NSScreen.visibleFrame` this layout was computed for (screen coordinates).
    public let visibleFrame: CGRect
    /// The panel's frame (screen coordinates), sized once for the largest state. Never resized or moved.
    public let panelFrame: CGRect
    /// The visual's 100 × 100 unit square (panel-local). The visual circle is inscribed in it.
    public let visualRect: CGRect
    /// `input.slot.facing`: radians (y-down) from the visual centre toward the visible frame's centre.
    public let facing: Double
    /// The information arc (normal mode).
    public let arc: InfoArc?
    /// The information line (compact mode).
    public let line: CompactLine?
    /// Union of everything that can be drawn, plus padding (panel-local).
    public let contentBounds: CGRect
    /// Translation (panel-local, y-down) of the content in the hidden state: beyond the
    /// visibleFrame edge. The slide animates between this and `.zero`.
    public let hiddenOffset: CGVector

    public var panelSize: CGSize { panelFrame.size }
    public var visualCenter: CGPoint { CGPoint(x: visualRect.midX, y: visualRect.midY) }
    public var visualRadius: CGFloat { visualRect.width / 2 }

    /// The visual square in screen coordinates (for pointer tracking and backdrop sampling).
    public var visualFrameOnScreen: CGRect {
        CGRect(x: panelFrame.minX + visualRect.minX, y: panelFrame.maxY - visualRect.maxY,
               width: visualRect.width, height: visualRect.height)
    }

    public func screenPoint(fromPanel point: CGPoint) -> CGPoint {
        CGPoint(x: panelFrame.minX + point.x, y: panelFrame.maxY - point.y)
    }

    public func panelPoint(fromScreen point: CGPoint) -> CGPoint {
        CGPoint(x: point.x - panelFrame.minX, y: panelFrame.maxY - point.y)
    }

    /// Converts a panel-local point into drawing units (may lie outside 0…100).
    public func drawingUnits(fromPanel point: CGPoint) -> CGPoint {
        CGPoint(x: (point.x - visualRect.minX) * 100 / visualRect.width,
                y: (point.y - visualRect.minY) * 100 / visualRect.height)
    }

    /// Converts a screen point (`NSEvent.mouseLocation`) into drawing units.
    public func drawingUnits(fromScreen point: CGPoint) -> CGPoint {
        drawingUnits(fromPanel: panelPoint(fromScreen: point))
    }

    /// The largest along-the-arc (or along-the-line) extent one element may take:
    /// one third of the space left after the gaps between three elements.
    public var maxElementAlongExtent: CGFloat {
        arc?.maxElementAlongExtent ?? line?.maxElementAlongExtent ?? 0
    }

    /// Lays show elements (or ask controls) out flex-centred along the arc or line.
    public func placeElements(_ requests: [ElementRequest]) -> [ElementPlacement] {
        if let arc { return arc.place(requests) }
        if let line { return line.place(requests) }
        return []
    }
}

/// How to shrink an element that is wider than a third of the arc.
public enum ElementFit: Sendable, Equatable {
    /// Scale uniformly (images).
    case scale
    /// Cut the along-the-arc dimension and keep the other (pills truncate their text).
    case truncate
}

public struct ElementRequest: Sendable, Equatable {
    /// Natural size, upright (width along the reading direction on horizontal arcs).
    public var size: CGSize
    public var fit: ElementFit

    public init(size: CGSize, fit: ElementFit) {
        self.size = size
        self.fit = fit
    }
}

/// Where one element goes. Draw it centred at `center`, rotated by `rotation`
/// (radians, clockwise positive in y-down coordinates), at `size`.
public struct ElementPlacement: Sendable, Equatable {
    public var center: CGPoint
    public var rotation: Double
    public var size: CGSize
    /// Position of the element's centre along the arc, from the arc's midpoint in reading order.
    public var arcLength: CGFloat
}

/// The information arc. Elements sit on the outer side of the baseline circle
/// (away from `center`, toward the screen centre), upright or following the
/// tangent, and never wider than ``maxElementAlongExtent``.
public struct InfoArc: Sendable, Equatable {
    /// Centre of the baseline circle (panel-local; usually outside the panel).
    public let center: CGPoint
    /// Radius of the baseline.
    public let radius: CGFloat
    /// Direction (y-down radians) from `center` to the arc's midpoint, i.e. toward the screen centre.
    public let normalAngle: Double
    public let halfSpan: Double
    /// Depth outward from the baseline the panel reserves for content (largest state).
    public let bandDepth: CGFloat
    /// Gap between neighbouring elements.
    public let gap: CGFloat
    /// +1 when reading order runs toward increasing angles.
    public let readingSign: Double
    /// Angle between an element's x-axis and the reading tangent: π/2 on the vertical
    /// left/right arcs (elements stay upright and stack top to bottom), otherwise 0.
    public let elementAxisOffset: Double

    public var length: CGFloat { radius * CGFloat(2 * halfSpan) }
    public var maxElementAlongExtent: CGFloat { max(0, (length - 2 * gap) / 3) }
    public var apex: CGPoint { point(atArcLength: 0) }
    public var startAngle: Double { normalAngle - readingSign * halfSpan }
    public var endAngle: Double { normalAngle + readingSign * halfSpan }

    /// Angle of the baseline point `s` points along the arc from the midpoint, in reading order.
    public func angle(atArcLength s: CGFloat) -> Double { normalAngle + readingSign * Double(s / radius) }

    /// A point on the baseline (or `offset` points outward from it).
    public func point(atArcLength s: CGFloat, offset: CGFloat = 0) -> CGPoint {
        let a = angle(atArcLength: s)
        let r = radius + offset
        return CGPoint(x: center.x + r * CGFloat(cos(a)), y: center.y + r * CGFloat(sin(a)))
    }

    /// Unit vector pointing away from `center` at `s`.
    public func outwardNormal(atArcLength s: CGFloat) -> CGVector {
        let a = angle(atArcLength: s)
        return CGVector(dx: cos(a), dy: sin(a))
    }

    /// Direction (y-down radians) of travel in reading order at `s`.
    public func tangentAngle(atArcLength s: CGFloat) -> Double {
        let a = angle(atArcLength: s)
        return atan2(readingSign * cos(a), -readingSign * sin(a))
    }

    /// Rotation for an element centred at `s` so it follows the arc while staying readable.
    public func elementRotation(atArcLength s: CGFloat) -> Double {
        tangentAngle(atArcLength: s) - elementAxisOffset
    }

    /// The same arc moved `offset` points outward (e.g. the question arc above the answer controls).
    public func concentric(offset: CGFloat) -> InfoArc {
        InfoArc(center: center, radius: radius + offset, normalAngle: normalAngle, halfSpan: halfSpan,
                bandDepth: max(0, bandDepth - offset), gap: gap, readingSign: readingSign,
                elementAxisOffset: elementAxisOffset)
    }

    /// Extent of an upright element along the arc and away from it.
    func extents(of size: CGSize) -> (along: CGFloat, depth: CGFloat) {
        let c = CGFloat(abs(cos(elementAxisOffset))), s = CGFloat(abs(sin(elementAxisOffset)))
        return (size.width * c + size.height * s, size.width * s + size.height * c)
    }

    func fitted(_ request: ElementRequest) -> CGSize {
        let limit = maxElementAlongExtent
        let along = extents(of: request.size).along
        guard along > limit, along > 0 else { return request.size }
        switch request.fit {
        case .scale:
            let factor = limit / along
            return CGSize(width: request.size.width * factor, height: request.size.height * factor)
        case .truncate:
            return elementAxisOffset == 0
                ? CGSize(width: limit, height: request.size.height)
                : CGSize(width: request.size.width, height: limit)
        }
    }

    /// Flex-centred placement along the baseline: elements keep reading order, each is at most a third of the arc.
    public func place(_ requests: [ElementRequest]) -> [ElementPlacement] {
        let sizes = requests.map(fitted)
        let extents = sizes.map { self.extents(of: $0) }
        let total = extents.reduce(0) { $0 + $1.along } + gap * CGFloat(max(0, sizes.count - 1))
        var cursor = -total / 2
        var placements: [ElementPlacement] = []
        for (size, extent) in zip(sizes, extents) {
            let s = cursor + extent.along / 2
            let base = point(atArcLength: s)
            let normal = outwardNormal(atArcLength: s)
            let lift = extent.depth / 2
            placements.append(
                ElementPlacement(
                    center: CGPoint(x: base.x + normal.dx * lift, y: base.y + normal.dy * lift),
                    rotation: elementRotation(atArcLength: s), size: size, arcLength: s))
            cursor += extent.along + gap
        }
        return placements
    }
}

/// The information line of compact mode: straight, horizontal, to the right of the small visual.
public struct CompactLine: Sendable, Equatable {
    public let start: CGPoint
    public let end: CGPoint
    /// Height of the content row centred on the line.
    public let contentHeight: CGFloat
    public let gap: CGFloat

    public var length: CGFloat { end.x - start.x }
    public var midpoint: CGPoint { CGPoint(x: (start.x + end.x) / 2, y: start.y) }
    public var maxElementAlongExtent: CGFloat { max(0, (length - 2 * gap) / 3) }

    /// Flex-centred placement along the line; elements are vertically centred on it.
    public func place(_ requests: [ElementRequest]) -> [ElementPlacement] {
        let limit = maxElementAlongExtent
        let sizes = requests.map { request -> CGSize in
            var size = request.size
            if size.height > contentHeight {
                size = CGSize(width: size.width * contentHeight / size.height, height: contentHeight)
            }
            guard size.width > limit, size.width > 0 else { return size }
            switch request.fit {
            case .scale: return CGSize(width: limit, height: size.height * limit / size.width)
            case .truncate: return CGSize(width: limit, height: size.height)
            }
        }
        let total = sizes.reduce(0) { $0 + $1.width } + gap * CGFloat(max(0, sizes.count - 1))
        var cursor = -total / 2
        return sizes.map { size in
            let s = cursor + size.width / 2
            cursor += size.width + gap
            return ElementPlacement(center: CGPoint(x: midpoint.x + s, y: midpoint.y), rotation: 0, size: size, arcLength: s)
        }
    }
}

// MARK: - Small geometry helpers

extension CGPoint {
    static func - (lhs: CGPoint, rhs: CGPoint) -> CGPoint { CGPoint(x: lhs.x - rhs.x, y: lhs.y - rhs.y) }
}

extension CGRect {
    /// Rounded outward to whole points so the window lands on pixel boundaries.
    var integralRect: CGRect {
        let minX = self.minX.rounded(.down), minY = self.minY.rounded(.down)
        return CGRect(x: minX, y: minY, width: self.maxX.rounded(.up) - minX, height: self.maxY.rounded(.up) - minY)
    }
}
