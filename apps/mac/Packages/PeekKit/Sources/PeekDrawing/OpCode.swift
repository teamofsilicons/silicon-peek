import Foundation

/// The op stream the prelude writes into `__peek_ops` (visual.md B3): `[opcode, argc, args…]` as doubles.
///
/// The prelude's opcode table is generated from this enum (``Prelude/source``), so the JavaScript and
/// Swift sides can never disagree. String arguments are indices into the frame's string table; enum
/// arguments are the small integers documented on each case.
enum OpCode: Int, CaseIterable, Sendable {
    // State and transform.
    case save = 1
    case restore = 2
    case translate = 3  // x y
    case rotate = 4  // angle
    case scale = 5  // x y
    case transform = 6  // a b c d e f
    case setTransform = 7  // a b c d e f
    case resetTransform = 8
    /// `ctx.reset()`: default state, empty stack, no clip, empty path.
    case reset = 9

    // Current path (points are transformed by the CTM as they are added, like canvas).
    case beginPath = 10
    case closePath = 11
    case moveTo = 12  // x y
    case lineTo = 13  // x y
    case arc = 14  // x y r a0 a1 ccw
    case arcTo = 15  // x1 y1 x2 y2 r
    case ellipse = 16  // x y rx ry rotation a0 a1 ccw
    case rect = 17  // x y w h
    case roundRect = 18  // x y w h  tlx tly trx try brx bry blx bly
    case quadraticCurveTo = 19  // cpx cpy x y
    case bezierCurveTo = 20  // cp1x cp1y cp2x cp2y x y

    // A Path2D, inlined before the paint op that uses it. Commands between begin and end build the
    // Path2D (in its own space); push/pop apply `addPath(p, matrix)` transforms inside the block.
    case pathPushTransform = 23  // a b c d e f
    case pathPopTransform = 24
    case path2DBegin = 25
    case path2DEnd = 26

    // Painting.
    case fill = 30  // rule usePath2D
    case stroke = 31  // usePath2D
    case clip = 32  // rule usePath2D
    case fillRect = 33  // x y w h
    case strokeRect = 34  // x y w h
    case clearRect = 35  // x y w h
    case fillText = 36  // text x y maxWidth(NaN = none)
    case strokeText = 37  // text x y maxWidth(NaN = none)
    case drawImage = 38  // id imageWidth imageHeight sx sy sw sh dx dy dw dh

    // Styles (recorded only when the value changes).
    case fillColor = 40  // css
    case strokeColor = 41  // css
    case fillGradient = 42  // gradient definition (see GradientSpec)
    case strokeGradient = 43  // gradient definition
    case lineWidth = 44
    case lineCap = 45  // 0 butt, 1 round, 2 square
    case lineJoin = 46  // 0 miter, 1 round, 2 bevel
    case miterLimit = 47
    case lineDash = 48  // segments…
    case lineDashOffset = 49
    case globalAlpha = 50
    case globalCompositeOperation = 51  // CompositeMode raw value
    case shadowColor = 52  // css
    case shadowBlur = 53
    case shadowOffsetX = 54
    case shadowOffsetY = 55
    case filterBlur = 56  // radius in units, 0 = none
    case font = 57  // css font shorthand
    case textAlign = 58  // TextAlign raw value
    case textBaseline = 59  // TextBaseline raw value

    // Peek extensions: layer breaks.
    case fillGlass = 70  // style tint(-1 = none) interactive rule usePath2D
    case fillBlur = 71  // material rule usePath2D
    case vibrant = 72  // 0/1

    /// Always the last op of a frame: ops recorded, ops dropped (5,000 cap or buffer), frame registered.
    case frameInfo = 99  // recorded dropped hasFrameFunction

    /// The name used in `--dump-frame` output (canvas-style, visual.md A10).
    var dumpName: String {
        switch self {
        case .save: "save"
        case .restore: "restore"
        case .translate: "translate"
        case .rotate: "rotate"
        case .scale: "scale"
        case .transform: "transform"
        case .setTransform: "setTransform"
        case .resetTransform: "resetTransform"
        case .reset: "reset"
        case .beginPath: "beginPath"
        case .closePath: "closePath"
        case .moveTo: "moveTo"
        case .lineTo: "lineTo"
        case .arc: "arc"
        case .arcTo: "arcTo"
        case .ellipse: "ellipse"
        case .rect: "rect"
        case .roundRect: "roundRect"
        case .quadraticCurveTo: "quadraticCurveTo"
        case .bezierCurveTo: "bezierCurveTo"
        case .pathPushTransform: "pathPushTransform"
        case .pathPopTransform: "pathPopTransform"
        case .path2DBegin: "path2DBegin"
        case .path2DEnd: "path2DEnd"
        case .fill: "fill"
        case .stroke: "stroke"
        case .clip: "clip"
        case .fillRect: "fillRect"
        case .strokeRect: "strokeRect"
        case .clearRect: "clearRect"
        case .fillText: "fillText"
        case .strokeText: "strokeText"
        case .drawImage: "drawImage"
        case .fillColor, .fillGradient: "fillStyle"
        case .strokeColor, .strokeGradient: "strokeStyle"
        case .lineWidth: "lineWidth"
        case .lineCap: "lineCap"
        case .lineJoin: "lineJoin"
        case .miterLimit: "miterLimit"
        case .lineDash: "setLineDash"
        case .lineDashOffset: "lineDashOffset"
        case .globalAlpha: "globalAlpha"
        case .globalCompositeOperation: "globalCompositeOperation"
        case .shadowColor: "shadowColor"
        case .shadowBlur: "shadowBlur"
        case .shadowOffsetX: "shadowOffsetX"
        case .shadowOffsetY: "shadowOffsetY"
        case .filterBlur: "filter"
        case .font: "font"
        case .textAlign: "textAlign"
        case .textBaseline: "textBaseline"
        case .fillGlass: "fillGlass"
        case .fillBlur: "fillBlur"
        case .vibrant: "vibrant"
        case .frameInfo: "frameInfo"
        }
    }

    /// The JavaScript identifier in the prelude's `OP` table.
    var jsName: String {
        switch self {
        case .fillColor: "fillColor"
        case .strokeColor: "strokeColor"
        case .fillGradient: "fillGradient"
        case .strokeGradient: "strokeGradient"
        case .lineDash: "lineDash"
        case .filterBlur: "filterBlur"
        default: dumpName
        }
    }

    /// The exact argument count, or `nil` when it varies (dash segments, gradients).
    var fixedArgumentCount: Int? {
        switch self {
        case .save, .restore, .resetTransform, .reset, .beginPath, .closePath, .pathPopTransform, .path2DBegin,
             .path2DEnd:
            0
        case .rotate, .lineWidth, .lineCap, .lineJoin, .miterLimit, .lineDashOffset, .globalAlpha,
             .globalCompositeOperation, .shadowColor, .shadowBlur, .shadowOffsetX, .shadowOffsetY, .filterBlur,
             .font, .textAlign, .textBaseline, .fillColor, .strokeColor, .vibrant, .stroke:
            1
        case .translate, .scale, .moveTo, .lineTo, .fill, .clip:
            2
        case .frameInfo, .fillBlur:
            3
        case .rect, .fillRect, .strokeRect, .clearRect, .quadraticCurveTo, .fillText, .strokeText:
            4
        case .arcTo, .fillGlass:
            5
        case .transform, .setTransform, .arc, .bezierCurveTo, .pathPushTransform:
            6
        case .ellipse:
            8
        case .drawImage:
            11
        case .roundRect:
            12
        case .lineDash, .fillGradient, .strokeGradient:
            nil
        }
    }

    /// Ops that build a path (the current path, or a Path2D inside a block).
    var isPathCommand: Bool {
        switch self {
        case .closePath, .moveTo, .lineTo, .arc, .arcTo, .ellipse, .rect, .roundRect, .quadraticCurveTo,
             .bezierCurveTo:
            true
        default:
            false
        }
    }
}

/// `globalCompositeOperation` values drawings may use (visual.md A5).
enum CompositeMode: Int, CaseIterable, Sendable {
    case sourceOver = 0
    case multiply = 1
    case screen = 2
    case overlay = 3
    case destinationOut = 4
    case lighter = 5

    var cssName: String {
        switch self {
        case .sourceOver: "source-over"
        case .multiply: "multiply"
        case .screen: "screen"
        case .overlay: "overlay"
        case .destinationOut: "destination-out"
        case .lighter: "lighter"
        }
    }
}

enum LineCapStyle: Int, CaseIterable, Sendable {
    case butt = 0
    case round = 1
    case square = 2

    var cssName: String { ["butt", "round", "square"][rawValue] }
}

enum LineJoinStyle: Int, CaseIterable, Sendable {
    case miter = 0
    case round = 1
    case bevel = 2

    var cssName: String { ["miter", "round", "bevel"][rawValue] }
}

enum TextAlign: Int, CaseIterable, Sendable {
    case start = 0
    case end = 1
    case left = 2
    case right = 3
    case center = 4

    var cssName: String { ["start", "end", "left", "right", "center"][rawValue] }
}

enum TextBaseline: Int, CaseIterable, Sendable {
    case alphabetic = 0
    case top = 1
    case hanging = 2
    case middle = 3
    case ideographic = 4
    case bottom = 5

    var cssName: String { ["alphabetic", "top", "hanging", "middle", "ideographic", "bottom"][rawValue] }
}

/// `fillGlass({style})`.
enum GlassStyle: Int, CaseIterable, Sendable {
    case regular = 0
    case clear = 1

    var cssName: String { self == .regular ? "regular" : "clear" }
}

/// `fillBlur({material})`.
enum BlurMaterial: Int, CaseIterable, Sendable {
    case hud = 0
    case popover = 1
    case menu = 2
    case sidebar = 3
    case underWindow = 4

    var cssName: String { ["hud", "popover", "menu", "sidebar", "underWindow"][rawValue] }
}

/// Gradient kinds in a `fillGradient` / `strokeGradient` definition:
/// `kind p0…p5 stopCount (offset, colorString)…` where linear uses `x0 y0 x1 y1 _ _`,
/// radial `x0 y0 r0 x1 y1 r1` and conic `startAngle x y _ _ _`.
enum GradientKind: Int, CaseIterable, Sendable {
    case linear = 0
    case radial = 1
    case conic = 2

    var cssName: String { ["linear", "radial", "conic"][rawValue] }
}

/// Drawing limits (visual.md A7, BLUEPRINT §0.1).
public enum DrawingLimits {
    /// Script size: rejected at register.
    public static let maxScriptBytes = 256 * 1024
    /// Time budget for one `frame()` call.
    public static let frameBudget: Duration = .milliseconds(4)
    /// Time budget for one event handler call.
    public static let eventBudget: Duration = .milliseconds(4)
    /// Time budget for the script's top-level code (not in A7; generous, but stops endless loops).
    public static let loadBudget: Duration = .milliseconds(250)
    /// Ops recorded per frame; the rest are ignored.
    public static let maxOpsPerFrame = 5_000
    /// Glass + blur fills per frame; extra ones are drawn as a flat translucent fill.
    public static let maxGlassFills = 3
    /// JS heap.
    public static let memoryLimitBytes = 16 << 20
    /// QuickJS stack limit; the VM thread's stack is 4 MiB (BLUEPRINT §8.6).
    public static let maxStackBytes = 1 << 20
    public static let threadStackBytes = 4 << 20
    /// Doubles in the op buffer (8 bytes each). 5,000 ops of up to 13 doubles fit comfortably.
    public static let opCapacity = 1 << 17
    /// Consecutive throwing frames before the fallback visual.
    public static let maxConsecutiveThrows = 10
    /// Overruns within ``overrunWindow`` before the fallback visual.
    public static let maxOverruns = 30
    public static let overrunWindow: Double = 5
    /// Glass outline/options changes are applied at most this often per bubble (D14).
    public static let glassUpdateInterval: Double = 0.1
    /// `input.dt` cap (visual.md B4).
    public static let maxDeltaTime: Double = 0.1
    /// Validation frames (visual.md A9).
    public static let validationFrames = 90
}
