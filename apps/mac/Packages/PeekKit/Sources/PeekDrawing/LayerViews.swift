import AppKit
import PeekCore
import QuartzCore
import SwiftUI

/// A glass outline in the drawing's 100 × 100 unit space, scaled to whatever rect SwiftUI gives it.
/// Stores a SwiftUI `Path` because `Shape` must be `Sendable` and `CGPath` is not (BLUEPRINT §8.3).
struct UnitPathShape: Shape {
    var unitPath: Path

    /// `glassEffect(in:)` fills with the nonzero rule, so every path is normalised first: `.evenOdd`
    /// turns an even-odd path into an equivalent nonzero one with real holes (gap-glass §3.3), and
    /// `.winding` removes self-overlaps so equal outlines hash and render the same.
    init(cgPath: CGPath, rule: CGPathFillRule) {
        unitPath = Path(cgPath.normalized(using: rule))
    }

    func path(in rect: CGRect) -> Path {
        unitPath.applying(CGAffineTransform(scaleX: rect.width / 100, y: rect.height / 100)
            .concatenating(CGAffineTransform(translationX: rect.minX, y: rect.minY)))
    }
}

/// One glass fill as SwiftUI sees it.
struct GlassShapeSpec: Equatable, Sendable {
    var shape: UnitPathShape
    var style: GlassStyle
    var tint: RGBA?
    var interactive: Bool

    static func == (lhs: GlassShapeSpec, rhs: GlassShapeSpec) -> Bool {
        lhs.shape.unitPath == rhs.shape.unitPath && lhs.style == rhs.style && lhs.tint == rhs.tint
            && lhs.interactive == rhs.interactive
    }

    func glass(frosted: Bool) -> Glass {
        // Frosted fallback (§8.4): non-key `clear` glass looks worse than `regular`, so map it.
        var glass: Glass = style == .clear && !frosted ? .clear : .regular
        if let tint {
            glass = glass.tint(Color(.sRGB, red: tint.red, green: tint.green, blue: tint.blue, opacity: tint.alpha))
        }
        return glass.interactive(interactive)
    }
}

/// The SwiftUI root of a glass host: one fill, or consecutive fills merged in a `GlassEffectContainer`
/// (D14: a container only for consecutive glass fills, never around draw layers).
struct GlassShapesView: View {
    var shapes: [GlassShapeSpec]
    var frosted: Bool

    var body: some View {
        if shapes.count == 1 {
            Color.clear.glassEffect(shapes[0].glass(frosted: frosted), in: shapes[0].shape)
        } else {
            GlassEffectContainer(spacing: 0) {
                ZStack {
                    ForEach(shapes.indices, id: \.self) { index in
                        Color.clear.glassEffect(shapes[index].glass(frosted: frosted), in: shapes[index].shape)
                    }
                }
            }
        }
    }
}

/// The flipped, clipped canvas a host puts inside PeekUI's visual view (visual.md B5: layer views in op
/// order, clipped to the 100 × 100 square).
final class DrawingCanvasView: NSView {
    weak var compositor: Compositor?

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        clipsToBounds = true
        layerContentsRedrawPolicy = .never
        autoresizingMask = [.width, .height]
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { nil }

    override var isFlipped: Bool { true }
    override var isOpaque: Bool { false }

    /// Clicks go to PeekUI's visual view (which turns them into `click` events), except over interactive
    /// glass, whose hosting view receives them so the glass can react.
    override func hitTest(_ point: NSPoint) -> NSView? {
        let local = convert(point, from: superview)
        return compositor?.interactiveGlassView(at: local)
    }

    override func layout() {
        super.layout()
        compositor?.canvasDidLayout()
    }
}

/// A `draw` layer: a layer-backed view whose `layer.contents` is the rendered bitmap.
final class DrawLayerView: NSView {
    private(set) var contentHash: Hash64?
    private(set) var image: CGImage?
    private var alpha: [UInt8]?

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        layerContentsRedrawPolicy = .never
        layer?.contentsGravity = .resize
        layer?.actions = ["contents": NSNull()]
        autoresizingMask = [.width, .height]
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { nil }

    override var isFlipped: Bool { true }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    func update(hash: Hash64, image: CGImage) {
        guard hash != contentHash else { return }
        contentHash = hash
        self.image = image
        alpha = nil
        layer?.contents = image
    }

    /// Alpha (0…1) of the bitmap under a unit point (B9 hit test), computed lazily per bitmap.
    func alpha(atUnit point: CGPoint) -> Double {
        guard let image else { return 0 }
        return Self.alpha(of: image, cache: &alpha, atUnit: point)
    }

    static func alpha(of image: CGImage, cache: inout [UInt8]?, atUnit point: CGPoint) -> Double {
        let width = image.width, height = image.height
        guard point.x >= 0, point.y >= 0, point.x < 100, point.y < 100, width > 0, height > 0 else { return 0 }
        if cache == nil {
            var buffer = [UInt8](repeating: 0, count: width * height)
            buffer.withUnsafeMutableBytes { raw in
                if let context = CGContext(data: raw.baseAddress, width: width, height: height, bitsPerComponent: 8,
                                           bytesPerRow: width, space: CGColorSpaceCreateDeviceGray(),
                                           bitmapInfo: CGImageAlphaInfo.alphaOnly.rawValue) {
                    context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
                }
            }
            cache = buffer
        }
        let x = min(width - 1, Int(point.x / 100 * Double(width)))
        let y = min(height - 1, Int(point.y / 100 * Double(height)))
        // Alpha-only contexts store rows top-down like any bitmap context's memory.
        return Double(cache![y * width + x]) / 255
    }
}

/// A `vibrant-draw` layer: the monochrome bitmap drawn by a view that allows vibrancy, inside an
/// `NSVisualEffectView` masked to the drawn pixels (notes/macos §1.3).
final class VibrantLayerView: NSVisualEffectView {
    private let content = VibrantContentView()
    private(set) var contentHash: Hash64?
    private var alphaCache: [UInt8]?

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        material = .popover
        blendingMode = .behindWindow
        state = .active
        autoresizingMask = [.width, .height]
        content.frame = bounds
        content.autoresizingMask = [.width, .height]
        addSubview(content)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { nil }

    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    func update(hash: Hash64, image: CGImage) {
        guard hash != contentHash else { return }
        contentHash = hash
        alphaCache = nil
        content.image = image
        content.needsDisplay = true
        maskImage = NSImage(cgImage: image, size: bounds.size == .zero ? NSSize(width: 100, height: 100) : bounds.size)
    }

    func alpha(atUnit point: CGPoint) -> Double {
        guard let image = content.image else { return 0 }
        return DrawLayerView.alpha(of: image, cache: &alphaCache, atUnit: point)
    }

    override func layout() {
        super.layout()
        if let image = content.image { maskImage = NSImage(cgImage: image, size: bounds.size) }
    }
}

final class VibrantContentView: NSView {
    var image: CGImage?

    override var allowsVibrancy: Bool { true }
    override var isFlipped: Bool { true }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    override func draw(_ dirtyRect: NSRect) {
        guard let image, let context = NSGraphicsContext.current?.cgContext else { return }
        context.saveGState()
        context.translateBy(x: 0, y: bounds.height)
        context.scaleBy(x: 1, y: -1)
        context.draw(image, in: bounds)
        context.restoreGState()
    }
}

/// A `glass` layer (or a run of consecutive ones): its own `NSHostingView` with
/// `Color.clear.glassEffect(…, in: UnitPathShape(localPath))`, inside a plain container whose layer carries
/// the CTM as an affine transform (D14, gap-glass §4 and §7.3). AppKit resets backing-layer transforms when
/// frames change, so the transform is re-applied after every layout and whenever it differs.
final class GlassLayerView: NSView {
    private let host: NSHostingView<GlassShapesView>
    private(set) var members: [GlassLayer] = []
    /// What the hosting view currently shows (outline + options + relative transforms).
    private(set) var shapeKey: Hash64?
    private(set) var pendingKey: Hash64?
    private var pendingShapes: [GlassShapeSpec]?
    private(set) var unitTransform: CGAffineTransform = .identity
    let frosted: Bool

    init(frame frameRect: NSRect, frosted: Bool) {
        self.frosted = frosted
        host = NSHostingView(rootView: GlassShapesView(shapes: [], frosted: frosted))
        super.init(frame: frameRect)
        wantsLayer = true
        autoresizingMask = [.width, .height]
        host.sizingOptions = []
        host.frame = bounds
        host.autoresizingMask = [.width, .height]
        addSubview(host)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { nil }

    override var isFlipped: Bool { true }

    var isInteractive: Bool { members.contains(where: \.interactive) }
    var hostingView: NSView { host }

    /// The key a set of members would show, their shapes, and their shared transform. The compositor only
    /// groups consecutive glass fills whose transforms are equal, so every member uses the first's.
    static func shapes(for members: [GlassLayer]) -> (key: Hash64, shapes: [GlassShapeSpec], transform: CGAffineTransform) {
        var key = Hash64(seed: 0x6147)
        let shapes = members.map { member -> GlassShapeSpec in
            key.mix(member.hash)
            return GlassShapeSpec(shape: UnitPathShape(cgPath: member.localPath, rule: member.rule), style: member.style,
                                  tint: member.tint, interactive: member.interactive)
        }
        return (key, shapes, members[0].transform)
    }

    /// Records the frame's members. Returns true when the outline/options differ from what is shown
    /// (the compositor then applies them under the 10 Hz limit); the transform applies immediately.
    func setMembers(_ members: [GlassLayer]) -> Bool {
        self.members = members
        let (key, shapes, transform) = Self.shapes(for: members)
        setUnitTransform(transform)
        guard key != shapeKey else {
            pendingKey = nil
            pendingShapes = nil
            return false
        }
        if key != pendingKey {
            pendingKey = key
            pendingShapes = shapes
        }
        return true
    }

    var hasPendingShape: Bool { pendingShapes != nil }

    /// Rebuilds the glass (the expensive part: SwiftUI re-renders the SDF).
    func applyPendingShape() {
        guard let shapes = pendingShapes, let key = pendingKey else { return }
        host.rootView = GlassShapesView(shapes: shapes, frosted: frosted)
        shapeKey = key
        pendingKey = nil
        pendingShapes = nil
    }

    func setUnitTransform(_ transform: CGAffineTransform) {
        unitTransform = transform
        applyTransform()
    }

    private func applyTransform() {
        guard let layer else { return }
        let k = bounds.width / 100
        let t = unitTransform
        let points = CGAffineTransform(a: t.a, b: t.b, c: t.c, d: t.d, tx: t.tx * k, ty: t.ty * k)
        guard layer.affineTransform() != points || layer.anchorPoint != .zero else { return }
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        // The canvas is flipped, so its sublayers are y-down with the origin at the top-left: with the
        // anchor at (0, 0) the unit transform maps directly (translation scaled to points).
        let origin = frame.origin
        layer.anchorPoint = .zero
        layer.position = origin
        layer.setAffineTransform(points)
        CATransaction.commit()
    }

    override func layout() {
        super.layout()
        applyTransform()
    }

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        applyTransform()
    }

    func contains(unitPoint: CGPoint) -> Bool { members.contains { $0.contains(unitPoint: unitPoint) } }
}

/// A `blur` layer: `NSVisualEffectView` masked to the path (notes/macos §1.3).
final class BlurLayerView: NSVisualEffectView {
    private(set) var appliedHash: Hash64?
    private(set) var layerSpec: BlurLayer?
    private var pending: BlurLayer?

    init(frame frameRect: NSRect, material: BlurMaterial) {
        super.init(frame: frameRect)
        self.material = Self.material(for: material)
        blendingMode = .behindWindow
        state = .active
        autoresizingMask = [.width, .height]
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { nil }

    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    static func material(for material: BlurMaterial) -> NSVisualEffectView.Material {
        switch material {
        case .hud: .hudWindow
        case .popover: .popover
        case .menu: .menu
        case .sidebar: .sidebar
        case .underWindow: .underWindowBackground
        }
    }

    /// Returns true when the mask must change (applied under the 10 Hz limit).
    func setSpec(_ spec: BlurLayer) -> Bool {
        layerSpec = spec
        guard spec.hash != appliedHash else {
            pending = nil
            return false
        }
        pending = spec
        return true
    }

    var hasPendingShape: Bool { pending != nil }

    func applyPendingShape() {
        guard let spec = pending else { return }
        material = Self.material(for: spec.material)
        applyMask(spec)
        appliedHash = spec.hash
        pending = nil
    }

    private func applyMask(_ spec: BlurLayer) {
        let normalized = spec.path.normalized(using: spec.rule)
        let size = bounds.size == .zero ? NSSize(width: 100, height: 100) : bounds.size
        maskImage = NSImage(size: size, flipped: true) { rect in
            guard let context = NSGraphicsContext.current?.cgContext else { return false }
            context.scaleBy(x: rect.width / 100, y: rect.height / 100)
            context.addPath(normalized)
            context.setFillColor(CGColor(gray: 0, alpha: 1))
            context.fillPath()
            return true
        }
    }

    override func layout() {
        super.layout()
        if let spec = layerSpec, appliedHash == spec.hash { applyMask(spec) }
    }
}

/// The fallback visual (visual.md A7, B10): a glass circle with the Silicon's initial in SF Pro Rounded.
struct FallbackVisual: View {
    var initial: String

    var body: some View {
        GeometryReader { geometry in
            let diameter = min(geometry.size.width, geometry.size.height) * 0.84
            ZStack {
                Color.clear
                    .glassEffect(.regular, in: Circle())
                    .frame(width: diameter, height: diameter)
                Text(initial)
                    .font(.system(size: diameter * 0.46, weight: .semibold, design: .rounded))
                    .foregroundStyle(.primary)
            }
            .frame(width: geometry.size.width, height: geometry.size.height)
        }
    }

    /// The first character of `name`, uppercased ("?" when empty).
    static func initial(from name: String) -> String {
        guard let first = name.trimmingCharacters(in: .whitespacesAndNewlines).first else { return "?" }
        return String(first).uppercased()
    }
}

final class FallbackVisualView: NSHostingView<FallbackVisual> {
    required init(rootView: FallbackVisual) {
        super.init(rootView: rootView)
        sizingOptions = []
        autoresizingMask = [.width, .height]
    }

    convenience init(initial: String) {
        self.init(rootView: FallbackVisual(initial: FallbackVisual.initial(from: initial)))
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { nil }

    override func hitTest(_ point: NSPoint) -> NSView? { nil }
}
