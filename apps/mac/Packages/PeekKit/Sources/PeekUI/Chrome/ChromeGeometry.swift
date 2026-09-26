import CoreGraphics
import Foundation
import PeekCore

// Small, pure geometry used by the chrome layout and its hit testing. Every point is
// panel-local (y-down, origin at the panel's top-left), the coordinate system of the
// chrome's SwiftUI view and of PeekCore's `SlotLayout`.

/// Wraps an angle into (−π, π].
@inline(__always)
func wrappedAngle(_ angle: Double) -> Double {
    var a = angle.truncatingRemainder(dividingBy: 2 * .pi)
    if a <= -.pi { a += 2 * .pi }
    if a > .pi { a -= 2 * .pi }
    return a
}

/// A rectangle of `size` centred at `center`, rotated by `rotation` radians (clockwise in y-down coordinates).
public struct RotatedRect: Sendable, Equatable {
    public var center: CGPoint
    public var size: CGSize
    public var rotation: Double

    public init(center: CGPoint, size: CGSize, rotation: Double) {
        self.center = center
        self.size = size
        self.rotation = rotation
    }

    /// The four corners, in panel coordinates.
    public var corners: [CGPoint] {
        let c = CGFloat(cos(rotation)), s = CGFloat(sin(rotation))
        let hw = size.width / 2, hh = size.height / 2
        return [(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)].map { dx, dy in
            CGPoint(x: center.x + dx * c - dy * s, y: center.y + dx * s + dy * c)
        }
    }

    /// Axis-aligned bounds of the rotated rectangle.
    public var boundingBox: CGRect {
        let points = corners
        let xs = points.map(\.x), ys = points.map(\.y)
        return CGRect(x: xs.min()!, y: ys.min()!, width: xs.max()! - xs.min()!, height: ys.max()! - ys.min()!)
    }

    /// Whether `point` lies inside, with `slop` points of tolerance on every side.
    public func contains(_ point: CGPoint, slop: CGFloat = 0) -> Bool {
        let dx = point.x - center.x, dy = point.y - center.y
        let c = CGFloat(cos(-rotation)), s = CGFloat(sin(-rotation))
        let lx = dx * c - dy * s, ly = dx * s + dy * c
        return abs(lx) <= size.width / 2 + slop && abs(ly) <= size.height / 2 + slop
    }
}

/// A curved band along a circle: the region between two radii over an angular span,
/// with round caps when drawn (hit testing includes the caps).
public struct ArcBand: Sendable, Equatable {
    public var center: CGPoint
    public var innerRadius: CGFloat
    public var outerRadius: CGFloat
    /// Start and end angles (y-down radians); `start` may be larger than `end`.
    public var startAngle: Double
    public var endAngle: Double

    public init(center: CGPoint, innerRadius: CGFloat, outerRadius: CGFloat, startAngle: Double, endAngle: Double) {
        self.center = center
        self.innerRadius = innerRadius
        self.outerRadius = outerRadius
        self.startAngle = startAngle
        self.endAngle = endAngle
    }

    public var thickness: CGFloat { outerRadius - innerRadius }
    public var midRadius: CGFloat { (innerRadius + outerRadius) / 2 }

    func point(angle: Double, radius: CGFloat) -> CGPoint {
        CGPoint(x: center.x + radius * CGFloat(cos(angle)), y: center.y + radius * CGFloat(sin(angle)))
    }

    public func contains(_ point: CGPoint, slop: CGFloat = 0) -> Bool {
        let dx = Double(point.x - center.x), dy = Double(point.y - center.y)
        let r = CGFloat((dx * dx + dy * dy).squareRoot())
        let lo = min(startAngle, endAngle), hi = max(startAngle, endAngle)
        let mid = (lo + hi) / 2
        let delta = wrappedAngle(atan2(dy, dx) - mid)
        if abs(delta) <= (hi - lo) / 2, r >= innerRadius - slop, r <= outerRadius + slop { return true }
        // Round caps.
        let capRadius = thickness / 2 + slop
        for angle in [startAngle, endAngle] {
            let cap = self.point(angle: angle, radius: midRadius)
            if hypot(point.x - cap.x, point.y - cap.y) <= capRadius { return true }
        }
        return false
    }

    /// A conservative axis-aligned bounding box (includes the caps).
    public var boundingBox: CGRect {
        let lo = min(startAngle, endAngle), hi = max(startAngle, endAngle)
        var rect = SlotGeometryHelpers.sectorBounds(center: center, inner: innerRadius, outer: outerRadius, from: lo, to: hi)
        let pad = thickness / 2
        rect = rect.insetBy(dx: -pad, dy: -pad)
        return rect
    }
}

/// A straight band (compact mode's line, or a straight track): a capsule from `start` to `end`.
public struct LineBand: Sendable, Equatable {
    public var start: CGPoint
    public var end: CGPoint
    public var thickness: CGFloat

    public init(start: CGPoint, end: CGPoint, thickness: CGFloat) {
        self.start = start
        self.end = end
        self.thickness = thickness
    }

    public var length: CGFloat { hypot(end.x - start.x, end.y - start.y) }

    public func contains(_ point: CGPoint, slop: CGFloat = 0) -> Bool {
        let vx = end.x - start.x, vy = end.y - start.y
        let lengthSquared = vx * vx + vy * vy
        var t: CGFloat = 0
        if lengthSquared > 0 { t = max(0, min(1, ((point.x - start.x) * vx + (point.y - start.y) * vy) / lengthSquared)) }
        let px = start.x + t * vx, py = start.y + t * vy
        return hypot(point.x - px, point.y - py) <= thickness / 2 + slop
    }
}

/// A region of the chrome that catches the pointer (visual.md B9 "over peek's own chrome").
public enum HitShape: Sendable, Equatable {
    case circle(center: CGPoint, radius: CGFloat)
    case rect(RotatedRect)
    case arc(ArcBand)
    case line(LineBand)
    case roundedRect(CGRect)

    public func contains(_ point: CGPoint, slop: CGFloat = 2) -> Bool {
        switch self {
        case .circle(let center, let radius): hypot(point.x - center.x, point.y - center.y) <= radius + slop
        case .rect(let rect): rect.contains(point, slop: slop)
        case .arc(let band): band.contains(point, slop: slop)
        case .line(let band): band.contains(point, slop: slop)
        case .roundedRect(let rect): rect.insetBy(dx: -slop, dy: -slop).contains(point)
        }
    }
}

/// Mirrors of internal PeekCore geometry helpers that the chrome needs (PeekCore keeps them internal).
enum SlotGeometryHelpers {
    /// Bounding box of an annular sector (y-down angles, `from` ≤ `to`).
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
}

extension InfoArc {
    /// Arc length (from the midpoint, in reading order) of the baseline point nearest to `point`.
    func arcLength(nearest point: CGPoint) -> CGFloat {
        let angle = atan2(Double(point.y - center.y), Double(point.x - center.x))
        return CGFloat(readingSign * wrappedAngle(angle - normalAngle)) * radius
    }

    /// Distance of `point` from the baseline circle's centre.
    func radialDistance(of point: CGPoint) -> CGFloat {
        hypot(point.x - center.x, point.y - center.y)
    }

    /// Extent of an upright element along the arc and away from it (PeekCore's rule).
    func alongAndDepth(of size: CGSize) -> (along: CGFloat, depth: CGFloat) {
        elementAxisOffset == 0 ? (size.width, size.height) : (size.height, size.width)
    }

    /// The band of the baseline circle between arc lengths `s0` and `s1`, from `inner` to `outer` points outward.
    func band(from s0: CGFloat, to s1: CGFloat, inner: CGFloat, outer: CGFloat) -> ArcBand {
        ArcBand(center: center, innerRadius: radius + inner, outerRadius: radius + outer,
                startAngle: angle(atArcLength: s0), endAngle: angle(atArcLength: s1))
    }
}
