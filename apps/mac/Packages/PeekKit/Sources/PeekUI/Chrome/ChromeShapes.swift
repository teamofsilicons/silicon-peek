import PeekCore
import SwiftUI

/// A curved band with round caps (the question band, the waveform band, slider tracks).
/// Coordinates are panel-local; `origin` is subtracted so the shape can live in a smaller frame.
struct ArcBandShape: Shape {
    var band: ArcBand
    var origin: CGPoint = .zero

    func path(in rect: CGRect) -> Path {
        var path = Path()
        guard band.thickness > 0 else { return path }
        let center = CGPoint(x: band.center.x - origin.x, y: band.center.y - origin.y)
        let a0 = min(band.startAngle, band.endAngle), a1 = max(band.startAngle, band.endAngle)
        let half = band.thickness / 2
        func point(_ angle: Double, _ radius: CGFloat) -> CGPoint {
            CGPoint(x: center.x + radius * CGFloat(cos(angle)), y: center.y + radius * CGFloat(sin(angle)))
        }
        path.addArc(center: center, radius: band.outerRadius, startAngle: .radians(a0), endAngle: .radians(a1), clockwise: false)
        path.addArc(center: point(a1, band.midRadius), radius: half, startAngle: .radians(a1), endAngle: .radians(a1 + .pi),
                    clockwise: false)
        path.addArc(center: center, radius: band.innerRadius, startAngle: .radians(a1), endAngle: .radians(a0), clockwise: true)
        path.addArc(center: point(a0, band.midRadius), radius: half, startAngle: .radians(a0 + .pi),
                    endAngle: .radians(a0 + 2 * .pi), clockwise: false)
        path.closeSubpath()
        return path
    }
}

/// A straight capsule from `start` to `end` (compact mode's tracks).
struct LineBandShape: Shape {
    var band: LineBand
    var origin: CGPoint = .zero

    func path(in rect: CGRect) -> Path {
        var path = Path()
        let start = CGPoint(x: band.start.x - origin.x, y: band.start.y - origin.y)
        let end = CGPoint(x: band.end.x - origin.x, y: band.end.y - origin.y)
        let length = hypot(end.x - start.x, end.y - start.y)
        let angle = atan2(end.y - start.y, end.x - start.x)
        let capsule = CGRect(x: -band.thickness / 2, y: -band.thickness / 2, width: length + band.thickness,
                             height: band.thickness)
        let transform = CGAffineTransform(translationX: start.x, y: start.y).rotated(by: angle)
        path.addRoundedRect(in: capsule, cornerSize: CGSize(width: band.thickness / 2, height: band.thickness / 2),
                            style: .continuous, transform: transform)
        return path
    }
}

/// The band of a track between fractions `from` and `to`. Animatable, so a band can grow from the
/// middle of the arc to its ends ("expands in X and takes up the entire space").
struct TrackShape: Shape {
    var track: ChromeLayout.Track
    var from: Double = 0
    var to: Double = 1
    var thickness: CGFloat
    var origin: CGPoint = .zero

    var animatableData: AnimatablePair<Double, Double> {
        get { AnimatablePair(from, to) }
        set {
            from = newValue.first
            to = newValue.second
        }
    }

    func path(in rect: CGRect) -> Path {
        let lo = min(from, to), hi = max(from, to)
        switch track {
        case .arc(let arc, let s0, let s1, let offset, _):
            let a = s0 + (s1 - s0) * CGFloat(lo), b = s0 + (s1 - s0) * CGFloat(hi)
            let band = arc.band(from: a, to: b, inner: offset - thickness / 2, outer: offset + thickness / 2)
            return ArcBandShape(band: band, origin: origin).path(in: rect)
        case .line(let start, let end, _):
            let p0 = CGPoint(x: start.x + (end.x - start.x) * CGFloat(lo), y: start.y + (end.y - start.y) * CGFloat(lo))
            let p1 = CGPoint(x: start.x + (end.x - start.x) * CGFloat(hi), y: start.y + (end.y - start.y) * CGFloat(hi))
            return LineBandShape(band: LineBand(start: p0, end: p1, thickness: thickness), origin: origin).path(in: rect)
        }
    }
}

extension ChromeLayout.Track {
    /// Unit vector pointing outward (away from the arc centre; up on a straight track) at fraction `t`.
    func normal(at t: Double) -> CGVector {
        switch self {
        case .arc(let arc, let s0, let s1, _, _):
            return arc.outwardNormal(atArcLength: s0 + (s1 - s0) * CGFloat(t))
        case .line:
            return CGVector(dx: 0, dy: -1)
        }
    }

    /// A conservative bounding box of the track band `thickness` wide.
    func boundingBox(thickness: CGFloat) -> CGRect {
        switch self {
        case .arc(let arc, let s0, let s1, let offset, _):
            return arc.band(from: s0, to: s1, inner: offset - thickness / 2, outer: offset + thickness / 2).boundingBox
        case .line(let start, let end, _):
            return CGRect(x: min(start.x, end.x), y: min(start.y, end.y), width: abs(end.x - start.x), height: abs(end.y - start.y))
                .insetBy(dx: -thickness / 2, dy: -thickness / 2)
        }
    }
}

/// A rounded rectangle, capsule or circle of `size`, offset by `offset` from the frame's centre in its own
/// (unrotated) coordinates, then rotated by `rotation` about the frame's centre: the path carries the rotation,
/// the view does not.
///
/// Liquid Glass must never sit under `rotationEffect`: on macOS 27 a `glassEffect` inside a rotated view is
/// rendered far larger than its frame (verified on screen: a 95 × 24 pt pill became a ~160 × 125 pt slab). So
/// every rotated piece of chrome glass is drawn by an unrotated view whose shape is this rotated path.
struct RotatedPlateShape: Shape {
    enum Corners: Equatable {
        case capsule
        case circle
        case radius(CGFloat)
    }

    var size: CGSize
    var offset: CGPoint = .zero
    var rotation: Double
    var corners: Corners
    /// Hover/press scale about the item's centre, and a screen-space shift (the hover lift). Animatable, so the glass
    /// follows the springy hover without a scale or rotation effect on a glass view.
    var scale: CGFloat = 1
    var shift: CGSize = .zero

    var animatableData: AnimatablePair<CGFloat, AnimatablePair<CGFloat, CGFloat>> {
        get { AnimatablePair(scale, AnimatablePair(shift.width, shift.height)) }
        set {
            scale = newValue.first
            shift = CGSize(width: newValue.second.first, height: newValue.second.second)
        }
    }

    func path(in rect: CGRect) -> Path {
        let local = CGRect(x: offset.x - size.width / 2, y: offset.y - size.height / 2, width: size.width, height: size.height)
        let path: Path =
            switch corners {
            case .capsule: Path(roundedRect: local, cornerRadius: min(size.width, size.height) / 2, style: .continuous)
            case .circle: Path(ellipseIn: local)
            case .radius(let radius):
                Path(roundedRect: local, cornerRadius: min(radius, min(size.width, size.height) / 2), style: .continuous)
            }
        let transform = CGAffineTransform(scaleX: scale, y: scale)
            .concatenating(CGAffineTransform(rotationAngle: CGFloat(rotation)))
            .concatenating(CGAffineTransform(translationX: rect.midX + shift.width, y: rect.midY + shift.height))
        return path.applying(transform)
    }
}

/// How a piece of chrome reacts to the pointer (ui-feedback.md #2, #3). Interactive chrome carries a faint rim and a
/// soft lift shadow at rest; on hover it springs up (scale, lift, brighter rim, bigger shadow) with an almost-bounce;
/// a press squashes it and the release springs back. Static chrome never reacts.
struct ChromeFeel: Equatable {
    var interactive = false
    var hovered = false
    var pressed = false
    var ink: Color = .white

    static let none = ChromeFeel()

    static let hoverScale: CGFloat = 1.07
    static let pressScale: CGFloat = 0.92
    static let hoverLift: CGFloat = 2
    /// "Almost bounce": underdamped, settles fast.
    static let spring = Animation.spring(response: 0.32, dampingFraction: 0.52)
    static let pressSpring = Animation.spring(response: 0.16, dampingFraction: 0.62)

    var scale: CGFloat {
        guard interactive else { return 1 }
        return pressed ? Self.pressScale : hovered ? Self.hoverScale : 1
    }

    var lift: CGFloat { interactive && hovered && !pressed ? Self.hoverLift : 0 }
    var rimOpacity: Double { hovered ? 0.7 : 0.26 }
    var rimWidth: CGFloat { hovered ? 1.6 : 1 }
    var shadowOpacity: Double { hovered ? 0.32 : 0.16 }
    var shadowRadius: CGFloat { hovered ? 7 : 2.5 }
    var shadowY: CGFloat { hovered ? 3.5 : 1.5 }
    var animation: Animation { pressed ? Self.pressSpring : Self.spring }
}

/// Reports a Button's pressed state to the view that draws its (separate) glass plate.
struct PressReportingStyle: ButtonStyle {
    @Binding var pressed: Bool

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .onChange(of: configuration.isPressed) { _, isPressed in pressed = isPressed }
    }
}

/// The glass behind (part of) a rotated row item: `rect` is in the item's own coordinates, origin at its centre.
struct GlassPlate: Equatable {
    var rect: CGRect
    var corners: RotatedPlateShape.Corners
    var tint: Color
    var interactive = false

    /// A plate covering an item of `size`.
    static func whole(_ size: CGSize, _ corners: RotatedPlateShape.Corners, tint: Color, interactive: Bool = false) -> GlassPlate {
        GlassPlate(rect: CGRect(x: -size.width / 2, y: -size.height / 2, width: size.width, height: size.height),
                   corners: corners, tint: tint, interactive: interactive)
    }

    /// A caption plate `height` tall along the bottom edge of an item of `size`.
    static func caption(itemSize size: CGSize, width: CGFloat? = nil, height: CGFloat, tint: Color) -> GlassPlate {
        let width = width ?? size.width
        return GlassPlate(rect: CGRect(x: -width / 2, y: size.height / 2 - height, width: width, height: height),
                          corners: .capsule, tint: tint)
    }
}

extension View {
    /// Places a view of `frame.size` centred at `frame.center` (panel-local), rotated by `frame.rotation` radians,
    /// over an optional glass `plate` that is drawn unrotated with a rotated path (see ``RotatedPlateShape``).
    /// `feel` adds the interactive rim, lift shadow and springy hover/press (ui-feedback.md #2, #3).
    func placed(_ frame: RotatedRect, plate: GlassPlate? = nil, glass: Bool = true, feel: ChromeFeel = .none) -> some View {
        ZStack(alignment: .topLeading) {
            if let plate {
                let box = RotatedRect(
                    center: frame.center,
                    size: CGSize(width: abs(plate.rect.midX) * 2 + plate.rect.width,
                                 height: abs(plate.rect.midY) * 2 + plate.rect.height),
                    rotation: frame.rotation
                ).boundingBox.insetBy(dx: -12, dy: -12)
                let shape = RotatedPlateShape(size: plate.rect.size, offset: CGPoint(x: plate.rect.midX, y: plate.rect.midY),
                                              rotation: frame.rotation, corners: plate.corners, scale: feel.scale,
                                              shift: CGSize(width: 0, height: -feel.lift))
                if feel.interactive {
                    shape.fill(Color.black.opacity(feel.shadowOpacity))
                        .frame(width: box.width, height: box.height)
                        .blur(radius: feel.shadowRadius)
                        .offset(y: feel.shadowY)
                        .position(frame.center)
                        .allowsHitTesting(false)
                }
                Color.clear
                    .frame(width: box.width, height: box.height)
                    .chromeGlass(shape, tint: plate.tint, glass: glass, interactive: plate.interactive)
                    .position(frame.center)
                    .allowsHitTesting(false)
                if feel.interactive {
                    shape.stroke(feel.ink.opacity(feel.rimOpacity), lineWidth: feel.rimWidth)
                        .frame(width: box.width, height: box.height)
                        .position(frame.center)
                        .allowsHitTesting(false)
                }
            }
            self.frame(width: frame.size.width, height: frame.size.height)
                .scaleEffect(feel.scale)
                .rotationEffect(.radians(frame.rotation))
                .position(frame.center)
                .offset(y: -feel.lift)
        }
        .animation(feel.animation, value: feel)
    }

    /// Liquid Glass tinted with the pill shade, or a flat translucent fill where glass would sit on glass
    /// (compact mode's strip), when Reduce Transparency is on, or when ``EnvironmentValues/chromeGlassEnabled`` is off.
    func chromeGlass<S: Shape>(_ shape: S, tint: Color, glass: Bool, interactive: Bool = false) -> some View {
        modifier(ChromeGlassModifier(shape: shape, tint: tint, glass: glass, interactive: interactive))
    }
}

extension EnvironmentValues {
    /// Draw the chrome's Liquid Glass (default). Off for offscreen rendering, which cannot sample a backdrop.
    @Entry public var chromeGlassEnabled = true
}

struct ChromeGlassModifier<S: Shape>: ViewModifier {
    let shape: S
    let tint: Color
    let glass: Bool
    let interactive: Bool
    @Environment(\.chromeGlassEnabled) private var glassEnabled
    @Environment(\.accessibilityReduceTransparency) private var reduceTransparency

    func body(content: Content) -> some View {
        if glass && glassEnabled && !reduceTransparency {
            content.glassEffect(Glass.regular.tint(tint).interactive(interactive), in: shape)
        } else if reduceTransparency {
            // Opaque: the tint over the window background, so nothing shows through.
            content.background(tint, in: shape).background(Color(nsColor: .windowBackgroundColor), in: shape)
        } else {
            content.background(tint, in: shape)
        }
    }
}
