import CoreGraphics
import Foundation

/// Builds a path with HTML canvas semantics (visual.md B6): every point is transformed by the transform
/// current when the command is added, so later transforms never move earlier points.
struct CanvasPath {
    private(set) var path = CGMutablePath()
    /// Running content hash (commands, arguments and the transform of each), for layer diffing.
    private(set) var hash = Hash64(seed: 0x5A5A_1234_ABCD_0001)
    /// The one transform every command since `reset` was added under, or `nil` if they differ (or none).
    private(set) var sharedTransform: CGAffineTransform?
    private(set) var mixedTransforms = false
    /// The commands in their own (untransformed) space, rebuilt lazily for glass (§0.1 item 6).
    private var local: [(OpCode, [Double])] = []

    var isEmpty: Bool { path.isEmpty }

    mutating func reset() {
        path = CGMutablePath()
        hash = Hash64(seed: 0x5A5A_1234_ABCD_0001)
        sharedTransform = nil
        mixedTransforms = false
        local.removeAll(keepingCapacity: true)
    }

    /// The path in its own space plus the transform to apply, when every command shared one transform.
    /// Otherwise the transformed path with the identity (the outline then changes with the transform).
    func glassGeometry() -> (path: CGPath, transform: CGAffineTransform, hash: Hash64) {
        guard let shared = sharedTransform, !mixedTransforms else {
            return (path.copy() ?? path, .identity, hash)
        }
        var rebuilt = CanvasPath()
        var localHash = Hash64(seed: 0x6B6B_5678_DCBA_0002)
        for (code, args) in local {
            rebuilt.apply(code, args, transform: .identity)
            localHash.mix(code.rawValue)
            for arg in args { localHash.mix(arg) }
        }
        return (rebuilt.path.copy() ?? rebuilt.path, shared, localHash)
    }

    /// Adds one path command. `args` must have the op's argument count (the decoder checks).
    mutating func apply(_ code: OpCode, _ args: [Double], transform t: CGAffineTransform) {
        hash.mix(code.rawValue)
        for arg in args { hash.mix(arg) }
        hash.mix(t)
        if local.isEmpty && path.isEmpty {
            sharedTransform = t
        } else if sharedTransform != t {
            mixedTransforms = true
        }
        local.append((code, args))

        switch code {
        case .moveTo:
            path.move(to: CGPoint(x: args[0], y: args[1]), transform: t)
        case .lineTo:
            lineTo(CGPoint(x: args[0], y: args[1]), t)
        case .closePath:
            if !path.isEmpty { path.closeSubpath() }
        case .quadraticCurveTo:
            ensureSubpath(CGPoint(x: args[0], y: args[1]), t)
            path.addQuadCurve(to: CGPoint(x: args[2], y: args[3]), control: CGPoint(x: args[0], y: args[1]),
                              transform: t)
        case .bezierCurveTo:
            ensureSubpath(CGPoint(x: args[0], y: args[1]), t)
            path.addCurve(to: CGPoint(x: args[4], y: args[5]), control1: CGPoint(x: args[0], y: args[1]),
                          control2: CGPoint(x: args[2], y: args[3]), transform: t)
        case .arc:
            arc(center: CGPoint(x: args[0], y: args[1]), radius: args[2], start: args[3], end: args[4],
                counterclockwise: args[5] != 0, t)
        case .arcTo:
            arcTo(CGPoint(x: args[0], y: args[1]), CGPoint(x: args[2], y: args[3]), radius: args[4], t)
        case .ellipse:
            ellipse(center: CGPoint(x: args[0], y: args[1]), radiusX: args[2], radiusY: args[3], rotation: args[4],
                    start: args[5], end: args[6], counterclockwise: args[7] != 0, t)
        case .rect:
            rect(x: args[0], y: args[1], width: args[2], height: args[3], t)
        case .roundRect:
            roundRect(x: args[0], y: args[1], width: args[2], height: args[3],
                      radii: [(args[4], args[5]), (args[6], args[7]), (args[8], args[9]), (args[10], args[11])], t)
        default:
            break
        }
    }

    // MARK: Commands

    private mutating func ensureSubpath(_ point: CGPoint, _ t: CGAffineTransform) {
        if path.isEmpty { path.move(to: point, transform: t) }
    }

    private mutating func lineTo(_ point: CGPoint, _ t: CGAffineTransform) {
        if path.isEmpty {
            path.move(to: point, transform: t)
        } else {
            path.addLine(to: point, transform: t)
        }
    }

    /// Canvas sweep rules: a full turn when the span is ≥ 2π in the drawing direction, otherwise the span
    /// reduced modulo 2π in that direction.
    static func sweep(start: Double, end: Double, counterclockwise: Bool) -> Double {
        let tau = 2 * Double.pi
        let delta = end - start
        if !counterclockwise {
            if delta >= tau { return tau }
            var d = delta.truncatingRemainder(dividingBy: tau)
            if d < 0 { d += tau }
            return d
        } else {
            if -delta >= tau { return -tau }
            var d = delta.truncatingRemainder(dividingBy: tau)
            if d > 0 { d -= tau }
            return d
        }
    }

    private mutating func arc(center: CGPoint, radius: Double, start: Double, end: Double, counterclockwise: Bool,
                              _ t: CGAffineTransform) {
        if radius == 0 {
            lineTo(center, t)
            return
        }
        let delta = Self.sweep(start: start, end: end, counterclockwise: counterclockwise)
        // CGPath's positive delta runs toward increasing angles, which is canvas's clockwise (y-down).
        path.addRelativeArc(center: center, radius: radius, startAngle: start, delta: delta, transform: t)
    }

    private mutating func ellipse(center: CGPoint, radiusX: Double, radiusY: Double, rotation: Double, start: Double,
                                  end: Double, counterclockwise: Bool, _ t: CGAffineTransform) {
        let delta = Self.sweep(start: start, end: end, counterclockwise: counterclockwise)
        let local = CGAffineTransform(scaleX: max(radiusX, 1e-9), y: max(radiusY, 1e-9))
            .concatenating(CGAffineTransform(rotationAngle: rotation))
            .concatenating(CGAffineTransform(translationX: center.x, y: center.y))
            .concatenating(t)
        path.addRelativeArc(center: .zero, radius: 1, startAngle: start, delta: delta, transform: local)
    }

    private mutating func arcTo(_ p1: CGPoint, _ p2: CGPoint, radius: Double, _ t: CGAffineTransform) {
        guard !path.isEmpty else {
            path.move(to: p1, transform: t)
            return
        }
        let inverse = t.inverted()
        guard t.a * t.d - t.b * t.c != 0 else {
            path.addLine(to: p1, transform: t)
            return
        }
        let p0 = path.currentPoint.applying(inverse)
        let v1 = CGPoint(x: p0.x - p1.x, y: p0.y - p1.y), v2 = CGPoint(x: p2.x - p1.x, y: p2.y - p1.y)
        let l1 = hypot(v1.x, v1.y), l2 = hypot(v2.x, v2.y)
        let cross = v1.x * v2.y - v1.y * v2.x
        if radius == 0 || l1 < 1e-12 || l2 < 1e-12 || abs(cross) < 1e-12 * l1 * l2 {
            path.addLine(to: p1, transform: t)
            return
        }
        let u1 = CGPoint(x: v1.x / l1, y: v1.y / l1), u2 = CGPoint(x: v2.x / l2, y: v2.y / l2)
        let cosAngle = max(-1, min(1, u1.x * u2.x + u1.y * u2.y))
        let angle = acos(cosAngle)
        let tangentDistance = radius / tan(angle / 2)
        let t1 = CGPoint(x: p1.x + u1.x * tangentDistance, y: p1.y + u1.y * tangentDistance)
        let t2 = CGPoint(x: p1.x + u2.x * tangentDistance, y: p1.y + u2.y * tangentDistance)
        var bisector = CGPoint(x: u1.x + u2.x, y: u1.y + u2.y)
        let bl = hypot(bisector.x, bisector.y)
        bisector = CGPoint(x: bisector.x / bl, y: bisector.y / bl)
        let centerDistance = radius / sin(angle / 2)
        let center = CGPoint(x: p1.x + bisector.x * centerDistance, y: p1.y + bisector.y * centerDistance)
        let a0 = atan2(t1.y - center.y, t1.x - center.x), a1 = atan2(t2.y - center.y, t2.x - center.x)
        var delta = a1 - a0
        while delta > .pi { delta -= 2 * .pi }
        while delta < -.pi { delta += 2 * .pi }
        path.addRelativeArc(center: center, radius: radius, startAngle: a0, delta: delta, transform: t)
    }

    private mutating func rect(x: Double, y: Double, width: Double, height: Double, _ t: CGAffineTransform) {
        path.move(to: CGPoint(x: x, y: y), transform: t)
        path.addLine(to: CGPoint(x: x + width, y: y), transform: t)
        path.addLine(to: CGPoint(x: x + width, y: y + height), transform: t)
        path.addLine(to: CGPoint(x: x, y: y + height), transform: t)
        path.closeSubpath()
        path.move(to: CGPoint(x: x, y: y), transform: t)
    }

    /// HTML `roundRect`: radii are (x, y) pairs for top-left, top-right, bottom-right, bottom-left.
    private mutating func roundRect(x: Double, y: Double, width: Double, height: Double,
                                    radii: [(Double, Double)], _ t: CGAffineTransform) {
        var x = x, y = y, w = width, h = height
        var (tl, tr, br, bl) = (radii[0], radii[1], radii[2], radii[3])
        if w < 0 {
            x += w
            w = -w
            swap(&tl, &tr)
            swap(&bl, &br)
        }
        if h < 0 {
            y += h
            h = -h
            swap(&tl, &bl)
            swap(&tr, &br)
        }
        func squared(_ r: (Double, Double)) -> (Double, Double) { r.0 <= 0 || r.1 <= 0 ? (0, 0) : r }
        tl = squared(tl)
        tr = squared(tr)
        br = squared(br)
        bl = squared(bl)
        var scale = 1.0
        for (sum, side) in [(tl.0 + tr.0, w), (tr.1 + br.1, h), (br.0 + bl.0, w), (tl.1 + bl.1, h)] where sum > 0 {
            scale = min(scale, side / sum)
        }
        if scale < 1 {
            tl = (tl.0 * scale, tl.1 * scale)
            tr = (tr.0 * scale, tr.1 * scale)
            br = (br.0 * scale, br.1 * scale)
            bl = (bl.0 * scale, bl.1 * scale)
        }
        func corner(_ cx: Double, _ cy: Double, _ r: (Double, Double), _ start: Double) {
            if r.0 <= 0 || r.1 <= 0 {
                path.addLine(to: CGPoint(x: cx, y: cy), transform: t)
                return
            }
            let local = CGAffineTransform(scaleX: r.0, y: r.1)
                .concatenating(CGAffineTransform(translationX: cx, y: cy))
                .concatenating(t)
            path.addRelativeArc(center: .zero, radius: 1, startAngle: start, delta: .pi / 2, transform: local)
        }
        // Each corner's arc adds the straight edge leading to it (CGPath joins the current point to the
        // arc's start), so no explicit lines are needed; square corners add a line to the corner point.
        path.move(to: CGPoint(x: x + tl.0, y: y), transform: t)
        corner(x + w - tr.0, y + tr.1, tr, -.pi / 2)
        corner(x + w - br.0, y + h - br.1, br, 0)
        corner(x + bl.0, y + h - bl.1, bl, .pi / 2)
        corner(x + tl.0, y + tl.1, tl, .pi)
        path.closeSubpath()
        path.move(to: CGPoint(x: x, y: y), transform: t)
    }
}
