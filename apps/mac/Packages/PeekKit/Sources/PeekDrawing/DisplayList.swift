import CoreGraphics
import Foundation
import PeekCore

/// A fill or stroke paint.
enum Paint: Hashable, Sendable {
    case color(RGBA)
    case gradient(GradientSpec)

    func mix(into hash: inout Hash64) {
        switch self {
        case .color(let color):
            hash.mix(1)
            color.mix(into: &hash)
        case .gradient(let gradient):
            hash.mix(2)
            gradient.mix(into: &hash)
        }
    }

    var monochrome: Paint {
        switch self {
        case .color(let color):
            return .color(color.monochrome)
        case .gradient(var gradient):
            gradient.stops = gradient.stops.map { GradientStop(offset: $0.offset, color: $0.color.monochrome) }
            return .gradient(gradient)
        }
    }
}

struct GradientStop: Hashable, Sendable {
    var offset: Double
    var color: RGBA
}

/// `createLinearGradient` / `createRadialGradient` / `createConicGradient` in the space current when the
/// paint is used (canvas semantics).
struct GradientSpec: Hashable, Sendable {
    var kind: GradientKind
    /// linear: x0 y0 x1 y1 · radial: x0 y0 r0 x1 y1 r1 · conic: startAngle x y.
    var params: [Double]
    var stops: [GradientStop]

    func mix(into hash: inout Hash64) {
        hash.mix(kind.rawValue)
        for p in params { hash.mix(p) }
        for stop in stops {
            hash.mix(stop.offset)
            stop.color.mix(into: &hash)
        }
    }
}

/// Canvas style state (everything `save()` saves except the transform and clip).
struct Style: Sendable {
    var fill: Paint = .color(.black)
    var stroke: Paint = .color(.black)
    var lineWidth: Double = 1
    var lineCap: LineCapStyle = .butt
    var lineJoin: LineJoinStyle = .miter
    var miterLimit: Double = 10
    var lineDash: [Double] = []
    var lineDashOffset: Double = 0
    var globalAlpha: Double = 1
    var composite: CompositeMode = .sourceOver
    var shadowColor: RGBA = .transparent
    var shadowBlur: Double = 0
    var shadowOffsetX: Double = 0
    var shadowOffsetY: Double = 0
    var filterBlur: Double = 0
    var font: FontSpec = .default
    var textAlign: TextAlign = .start
    var textBaseline: TextBaseline = .alphabetic

    var hasShadow: Bool {
        shadowColor.alpha > 0 && (shadowBlur > 0 || shadowOffsetX != 0 || shadowOffsetY != 0)
    }

    func mix(into hash: inout Hash64) {
        fill.mix(into: &hash)
        stroke.mix(into: &hash)
        hash.mix(lineWidth)
        hash.mix(lineCap.rawValue)
        hash.mix(lineJoin.rawValue)
        hash.mix(miterLimit)
        hash.mix(lineDash.count)
        for d in lineDash { hash.mix(d) }
        hash.mix(lineDashOffset)
        hash.mix(globalAlpha)
        hash.mix(composite.rawValue)
        shadowColor.mix(into: &hash)
        hash.mix(shadowBlur)
        hash.mix(shadowOffsetX)
        hash.mix(shadowOffsetY)
        hash.mix(filterBlur)
        hash.mix(font.family.rawValue)
        hash.mix(font.weight)
        hash.mix(font.italic)
        hash.mix(font.size)
        hash.mix(textAlign.rawValue)
        hash.mix(textBaseline.rawValue)
    }
}

/// One `clip()` in the current clip stack (paths are in unit space; clips intersect).
final class ClipNode: @unchecked Sendable {
    let path: CGPath
    let rule: CGPathFillRule
    let parent: ClipNode?
    let hash: Hash64

    init(path: CGPath, rule: CGPathFillRule, parent: ClipNode?, contentHash: Hash64) {
        self.path = path
        self.rule = rule
        self.parent = parent
        var h = parent?.hash ?? Hash64(seed: 0xC11F)
        h.mix(contentHash)
        h.mix(rule == .evenOdd)
        self.hash = h
    }

    /// Root-first.
    var chain: [ClipNode] {
        var nodes: [ClipNode] = []
        var node: ClipNode? = self
        while let current = node {
            nodes.append(current)
            node = current.parent
        }
        return nodes.reversed()
    }
}

/// A resolved paint operation, ready to replay without further state (visual.md B6).
struct DrawCommand: @unchecked Sendable {
    enum Kind {
        /// Path in unit space.
        case fill(CGPath, CGPathFillRule)
        /// Path in unit space; the replayer strokes it in the space of `ctm` (canvas line widths).
        case stroke(CGPath)
        /// Path in unit space.
        case clear(CGPath)
        case text(String, x: Double, y: Double, maxWidth: Double?, stroke: Bool)
        /// `source` is in the image's declared pixel space (handle width/height).
        case image(id: Int, declaredSize: CGSize, source: CGRect, destination: CGRect)
        /// A glass/blur fill beyond the per-frame cap, drawn as a flat translucent fill (A7).
        case flatGlass(CGPath, CGPathFillRule, tint: RGBA?)
    }

    var kind: Kind
    var ctm: CGAffineTransform
    var style: Style
    var clip: ClipNode?
}

/// A run of ordinary drawing between layer breaks.
struct DrawLayer: @unchecked Sendable {
    var vibrant: Bool
    var commands: [DrawCommand]
    var hash: Hash64
    /// The canvas-style ops of this layer for `--dump-frame` (only recorded when asked).
    var dumpOps: [JSONValue]?
}

/// A `fillGlass` layer. The path is local; `transform` is the CTM at the fill (BLUEPRINT §0.1 item 6).
struct GlassLayer: @unchecked Sendable {
    var localPath: CGPath
    var rule: CGPathFillRule
    var style: GlassStyle
    var tint: RGBA?
    var tintCSS: String?
    var interactive: Bool
    var transform: CGAffineTransform
    /// Local path + rule: what rebuilding the glass costs.
    var outlineHash: Hash64
    /// Style, tint and interactivity.
    var optionsHash: Hash64

    var hash: Hash64 {
        var h = outlineHash
        h.mix(optionsHash)
        return h
    }

    /// The outline in unit space.
    var unitPath: CGPath {
        var t = transform
        return localPath.copy(using: &t) ?? localPath
    }

    func contains(unitPoint: CGPoint) -> Bool {
        let det = transform.a * transform.d - transform.b * transform.c
        guard det != 0 else { return false }
        return localPath.contains(unitPoint.applying(transform.inverted()), using: rule)
    }
}

/// A `fillBlur` layer (unit-space path).
struct BlurLayer: @unchecked Sendable {
    var path: CGPath
    var rule: CGPathFillRule
    var material: BlurMaterial
    var hash: Hash64

    func contains(unitPoint: CGPoint) -> Bool { path.contains(unitPoint, using: rule) }
}

enum DisplayLayer: @unchecked Sendable {
    case draw(DrawLayer)
    case glass(GlassLayer)
    case blur(BlurLayer)

    var kindName: String {
        switch self {
        case .draw(let layer): layer.vibrant ? "vibrant-draw" : "draw"
        case .glass: "glass"
        case .blur: "blur"
        }
    }
}

/// One frame after splitting (visual.md B5).
struct DisplayList: @unchecked Sendable {
    var layers: [DisplayLayer] = []
    var again = false
    /// Ops the prelude recorded / dropped (5,000 cap).
    var recordedOps = 0
    var droppedOps = 0
    /// Whether the script registered `peek.frame`.
    var hasFrameFunction = true
    /// Glass/blur fills beyond the cap of 3, drawn flat.
    var glassOverflow = 0
    /// Paint operations that can put pixels on screen (fills, strokes, text, images, glass, blur).
    var paintCount = 0
    var textCount = 0
    /// Problems found while decoding (unknown colours, fonts, malformed ops), each once per decoder.
    var diagnostics: [String] = []

    var glassLayerCount: Int {
        layers.reduce(0) { count, layer in
            switch layer {
            case .glass, .blur: count + 1
            case .draw: count
            }
        }
    }
}
