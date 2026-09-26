import CoreGraphics
import Foundation
import PeekCore

/// Turns one frame's op stream into a ``DisplayList`` (visual.md B5 "split"): simulates canvas state
/// (styles, transform, clip, save stack, current path, Path2D blocks), resolves every paint op into a
/// self-contained ``DrawCommand``, starts a new layer at every `fillGlass`, `fillBlur` and change of
/// `ctx.vibrant`, and hashes each layer for the compositor's diff.
///
/// The op stream comes from untrusted script code (a drawing can monkey-patch the prelude's built-ins), so
/// every opcode, argument count, number and string index is checked; bad ops are skipped and reported once
/// (B10 "unknown op or bad args → ignore that op, log once").
///
/// One decoder per drawing; it lives on the drawing's ``JSThread``.
final class OpDecoder {
    /// Reported once when a frame draws more than 3 glass/blur fills (validation turns it into `glass_limit`).
    static let glassOverflowMessage = "more than \(DrawingLimits.maxGlassFills) glass/blur fills in one frame; "
        + "the extra ones are drawn as a flat translucent fill (\(DrawingDocs.limits))"

    private let fonts: FontCache
    private var colors: [String: RGBA?] = [:]
    private var reported: Set<String> = []

    init(fonts: FontCache = .shared) {
        self.fonts = fonts
    }

    private struct GState {
        var style = Style()
        var ctm = CGAffineTransform.identity
        var clip: ClipNode?
    }

    /// Decodes one frame. `images` are the frame's live image handles (their identity joins the layer hash).
    func decode(ops: [Double], strings: [String], again: Bool, images: [Int: CGImage] = [:],
                recordDump: Bool = false) -> DisplayList {
        var run = Run(decoder: self, strings: strings, images: images, recordDump: recordDump)
        run.list.again = again
        run.execute(ops)
        return run.finish()
    }

    fileprivate func color(_ css: String) -> RGBA? {
        if let cached = colors[css] { return cached }
        let parsed = CSSColor.parse(css)
        if colors.count > 512 { colors.removeAll(keepingCapacity: true) }
        colors[css] = parsed
        return parsed
    }

    fileprivate func fontSpec(_ css: String) -> FontSpec? { fonts.spec(for: css) }

    /// True the first time `message` is reported by this decoder.
    fileprivate func firstReport(_ message: String) -> Bool { reported.insert(message).inserted }

    // MARK: - One frame

    private struct Run {
        let decoder: OpDecoder
        let strings: [String]
        let images: [Int: CGImage]
        let recordDump: Bool

        var list = DisplayList()
        var state = GState()
        var stack: [GState] = []
        var path = CanvasPath()
        var block: CanvasPath?
        var blockTransforms: [CGAffineTransform] = []
        var readyBlock: CanvasPath?
        var readyBlockDump: [JSONValue] = []
        var vibrant = false

        var commands: [DrawCommand] = []
        var layerHash = Hash64()
        var dumpOps: [JSONValue] = []
        var stringHashes: [Hash64?]

        init(decoder: OpDecoder, strings: [String], images: [Int: CGImage], recordDump: Bool) {
            self.decoder = decoder
            self.strings = strings
            self.images = images
            self.recordDump = recordDump
            stringHashes = Array(repeating: nil, count: strings.count)
            layerHash = initialHash()
        }

        mutating func finish() -> DisplayList {
            closeDrawLayer()
            return list
        }

        // MARK: Diagnostics

        mutating func report(_ message: String) {
            if decoder.firstReport(message) { list.diagnostics.append(message) }
        }

        // MARK: Layers

        func initialHash() -> Hash64 {
            var h = Hash64(seed: vibrant ? 0xB1B1 : 0xD1D1)
            state.style.mix(into: &h)
            h.mix(state.ctm)
            h.mix(state.clip?.hash.value ?? 0)
            h.mix(stack.count)
            for saved in stack {
                saved.style.mix(into: &h)
                h.mix(saved.ctm)
                h.mix(saved.clip?.hash.value ?? 0)
            }
            return h
        }

        mutating func closeDrawLayer() {
            if !commands.isEmpty {
                list.layers.append(.draw(DrawLayer(vibrant: vibrant, commands: commands, hash: layerHash,
                                                   dumpOps: recordDump ? dumpOps : nil)))
            }
            commands.removeAll(keepingCapacity: true)
            dumpOps.removeAll(keepingCapacity: true)
            layerHash = initialHash()
        }

        mutating func add(_ kind: DrawCommand.Kind) {
            commands.append(DrawCommand(kind: kind, ctm: state.ctm, style: state.style, clip: state.clip))
        }

        // MARK: Arguments

        mutating func string(_ value: Double, op: OpCode) -> (String, Hash64)? {
            guard value.isFinite, value >= 0, value < Double(strings.count), value == value.rounded() else {
                report("\(op.dumpName): string index \(value) is out of range; the op was ignored")
                return nil
            }
            let index = Int(value)
            if let hash = stringHashes[index] { return (strings[index], hash) }
            var hash = Hash64(seed: 0x57)
            hash.mix(strings[index])
            stringHashes[index] = hash
            return (strings[index], hash)
        }

        func dumpNumber(_ value: Double) -> JSONValue {
            guard value.isFinite else { return .null }
            let rounded = (value * 1000).rounded() / 1000
            if rounded == rounded.rounded(), abs(rounded) < 1e15 { return .int(Int64(rounded)) }
            return .double(rounded)
        }

        mutating func dump(_ name: String, _ args: [JSONValue]) {
            guard recordDump else { return }
            dumpOps.append(.array([.string(name)] + args))
        }

        // MARK: Execution

        mutating func execute(_ ops: [Double]) {
            var i = 0
            let count = ops.count
            while i + 1 < count {
                let rawCode = ops[i], rawArgc = ops[i + 1]
                guard rawCode.isFinite, rawArgc.isFinite, rawArgc >= 0, rawArgc == rawArgc.rounded(),
                      Double(i) + 2 + rawArgc <= Double(count)
                else {
                    report("the op stream is malformed at position \(i); the rest of the frame was ignored")
                    return
                }
                let argc = Int(rawArgc)
                let base = i + 2
                i = base + argc
                guard rawCode == rawCode.rounded(), let op = OpCode(rawValue: Int(rawCode)) else {
                    report("unknown op \(rawCode) was ignored")
                    continue
                }
                if let fixed = op.fixedArgumentCount, fixed != argc {
                    report("\(op.dumpName) has \(argc) arguments instead of \(fixed); the op was ignored")
                    continue
                }
                let args = ops[base..<(base + argc)]
                if op != .frameInfo, !args.allSatisfy(\.isFinite), op != .fillText, op != .strokeText {
                    report("\(op.dumpName) has a non-finite argument; the op was ignored")
                    continue
                }
                apply(op, Array(args))
            }
        }

        // swiftlint:disable:next cyclomatic_complexity function_body_length
        mutating func apply(_ op: OpCode, _ a: [Double]) {
            switch op {
            // State and transform.
            case .save:
                stack.append(state)
                mixRaw(op, a)
                dump("save", [])
            case .restore:
                if let saved = stack.popLast() { state = saved }
                mixRaw(op, a)
                dump("restore", [])
            case .translate:
                state.ctm = state.ctm.translatedBy(x: a[0], y: a[1])
                mixRaw(op, a)
                dump("translate", a.map(dumpNumber))
            case .rotate:
                state.ctm = state.ctm.rotated(by: a[0])
                mixRaw(op, a)
                dump("rotate", a.map(dumpNumber))
            case .scale:
                state.ctm = state.ctm.scaledBy(x: a[0], y: a[1])
                mixRaw(op, a)
                dump("scale", a.map(dumpNumber))
            case .transform:
                state.ctm = CGAffineTransform(a: a[0], b: a[1], c: a[2], d: a[3], tx: a[4], ty: a[5])
                    .concatenating(state.ctm)
                mixRaw(op, a)
                dump("transform", a.map(dumpNumber))
            case .setTransform:
                state.ctm = CGAffineTransform(a: a[0], b: a[1], c: a[2], d: a[3], tx: a[4], ty: a[5])
                mixRaw(op, a)
                dump("setTransform", a.map(dumpNumber))
            case .resetTransform:
                state.ctm = .identity
                mixRaw(op, a)
                dump("resetTransform", [])
            case .reset:
                state = GState()
                stack.removeAll()
                path.reset()
                mixRaw(op, a)
                dump("reset", [])

            // Paths.
            case .beginPath:
                if block != nil {
                    report("beginPath inside a Path2D block was ignored")
                    return
                }
                path.reset()
                dump("beginPath", [])
            case .closePath, .moveTo, .lineTo, .arc, .arcTo, .ellipse, .rect, .roundRect, .quadraticCurveTo,
                 .bezierCurveTo:
                if block != nil {
                    block!.apply(op, a, transform: blockTransforms.last ?? .identity)
                    if recordDump { readyBlockDump.append(.array([.string(op.dumpName)] + a.map(dumpNumber))) }
                } else {
                    path.apply(op, a, transform: state.ctm)
                    dump(op.dumpName, a.map(dumpNumber))
                }
            case .pathPushTransform:
                guard block != nil else {
                    report("pathPushTransform outside a Path2D block was ignored")
                    return
                }
                let m = CGAffineTransform(a: a[0], b: a[1], c: a[2], d: a[3], tx: a[4], ty: a[5])
                blockTransforms.append(m.concatenating(blockTransforms.last ?? .identity))
            case .pathPopTransform:
                if block != nil, !blockTransforms.isEmpty { blockTransforms.removeLast() }
            case .path2DBegin:
                block = CanvasPath()
                blockTransforms.removeAll()
                readyBlock = nil
                readyBlockDump.removeAll()
            case .path2DEnd:
                readyBlock = block
                block = nil

            // Painting.
            case .fill:
                let rule: CGPathFillRule = a[0] == 1 ? .evenOdd : .winding
                guard let (unit, hash) = consumePath(usePath2D: a[1] == 1) else { return }
                layerHash.mix(op.rawValue)
                layerHash.mix(rule == .evenOdd)
                layerHash.mix(hash)
                add(.fill(unit, rule))
                list.paintCount += 1
                dump("fill", [.string(rule == .evenOdd ? "evenodd" : "nonzero")] + blockDumpArgument(a[1] == 1))
            case .stroke:
                guard let (unit, hash) = consumePath(usePath2D: a[0] == 1) else { return }
                layerHash.mix(op.rawValue)
                layerHash.mix(hash)
                layerHash.mix(state.ctm)
                add(.stroke(unit))
                list.paintCount += 1
                dump("stroke", blockDumpArgument(a[0] == 1))
            case .clip:
                let rule: CGPathFillRule = a[0] == 1 ? .evenOdd : .winding
                let usePath2D = a[1] == 1
                guard let (unit, hash) = consumePath(usePath2D: usePath2D, allowEmpty: true) else { return }
                state.clip = ClipNode(path: unit, rule: rule, parent: state.clip, contentHash: hash)
                layerHash.mix(op.rawValue)
                layerHash.mix(state.clip!.hash)
                dump("clip", [.string(rule == .evenOdd ? "evenodd" : "nonzero")] + blockDumpArgument(usePath2D))
            case .fillRect, .strokeRect, .clearRect:
                let rect = CGRect(x: a[0], y: a[1], width: a[2], height: a[3])
                let unit = CGMutablePath()
                unit.addRect(rect, transform: state.ctm)
                mixRaw(op, a)
                layerHash.mix(state.ctm)
                switch op {
                case .fillRect:
                    guard a[2] != 0, a[3] != 0 else { return }
                    add(.fill(unit, .winding))
                    list.paintCount += 1
                case .strokeRect:
                    guard a[2] != 0 || a[3] != 0 else { return }
                    add(.stroke(unit))
                    list.paintCount += 1
                default:
                    add(.clear(unit))
                }
                dump(op.dumpName, a.map(dumpNumber))
            case .fillText, .strokeText:
                guard a[1].isFinite, a[2].isFinite, let (text, textHash) = string(a[0], op: op) else { return }
                let maxWidth: Double? = a[3].isFinite && a[3] > 0 ? a[3] : nil
                layerHash.mix(op.rawValue)
                layerHash.mix(textHash)
                layerHash.mix(a[1])
                layerHash.mix(a[2])
                layerHash.mix(maxWidth ?? -1)
                layerHash.mix(state.ctm)
                add(.text(text, x: a[1], y: a[2], maxWidth: maxWidth, stroke: op == .strokeText))
                if !text.isEmpty {
                    list.paintCount += 1
                    list.textCount += 1
                }
                dump(op.dumpName, [.string(text), dumpNumber(a[1]), dumpNumber(a[2])]
                    + (maxWidth.map { [dumpNumber($0)] } ?? []))
            case .drawImage:
                drawImage(a)

            // Styles.
            case .fillColor, .strokeColor, .shadowColor:
                guard let (css, hash) = string(a[0], op: op) else { return }
                layerHash.mix(op.rawValue)
                layerHash.mix(hash)
                dump(op.dumpName, [.string(css)])
                guard let color = decoder.color(css) else {
                    report("\(op.dumpName) \"\(css)\" is not a CSS color; the previous value was kept")
                    return
                }
                switch op {
                case .fillColor: state.style.fill = .color(color)
                case .strokeColor: state.style.stroke = .color(color)
                default: state.style.shadowColor = color
                }
            case .fillGradient, .strokeGradient:
                guard let gradient = gradient(a, op: op) else { return }
                var hash = Hash64(seed: 0x6A)
                gradient.mix(into: &hash)
                layerHash.mix(op.rawValue)
                layerHash.mix(hash)
                if op == .fillGradient { state.style.fill = .gradient(gradient) } else {
                    state.style.stroke = .gradient(gradient)
                }
                dump(op.dumpName, [gradientDump(gradient)])
            case .lineWidth:
                guard a[0] > 0 else { return }
                state.style.lineWidth = a[0]
                mixRaw(op, a)
                dump(op.dumpName, [dumpNumber(a[0])])
            case .lineCap:
                guard let cap = LineCapStyle(rawValue: Int(a[0])) else { return }
                state.style.lineCap = cap
                mixRaw(op, a)
                dump(op.dumpName, [.string(cap.cssName)])
            case .lineJoin:
                guard let join = LineJoinStyle(rawValue: Int(a[0])) else { return }
                state.style.lineJoin = join
                mixRaw(op, a)
                dump(op.dumpName, [.string(join.cssName)])
            case .miterLimit:
                guard a[0] > 0 else { return }
                state.style.miterLimit = a[0]
                mixRaw(op, a)
                dump(op.dumpName, [dumpNumber(a[0])])
            case .lineDash:
                guard a.allSatisfy({ $0 >= 0 }) else { return }
                state.style.lineDash = a.count % 2 == 1 ? a + a : a
                mixRaw(op, a)
                dump(op.dumpName, [.array(a.map(dumpNumber))])
            case .lineDashOffset:
                state.style.lineDashOffset = a[0]
                mixRaw(op, a)
                dump(op.dumpName, [dumpNumber(a[0])])
            case .globalAlpha:
                guard (0...1).contains(a[0]) else { return }
                state.style.globalAlpha = a[0]
                mixRaw(op, a)
                dump(op.dumpName, [dumpNumber(a[0])])
            case .globalCompositeOperation:
                guard let mode = CompositeMode(rawValue: Int(a[0])) else { return }
                state.style.composite = mode
                mixRaw(op, a)
                dump(op.dumpName, [.string(mode.cssName)])
            case .shadowBlur:
                guard a[0] >= 0 else { return }
                state.style.shadowBlur = a[0]
                mixRaw(op, a)
                dump(op.dumpName, [dumpNumber(a[0])])
            case .shadowOffsetX:
                state.style.shadowOffsetX = a[0]
                mixRaw(op, a)
                dump(op.dumpName, [dumpNumber(a[0])])
            case .shadowOffsetY:
                state.style.shadowOffsetY = a[0]
                mixRaw(op, a)
                dump(op.dumpName, [dumpNumber(a[0])])
            case .filterBlur:
                state.style.filterBlur = max(0, a[0])
                mixRaw(op, a)
                dump(op.dumpName, [.string(a[0] > 0 ? "blur(\(dumpNumber(a[0]).jsonString)px)" : "none")])
            case .font:
                guard let (css, hash) = string(a[0], op: op) else { return }
                layerHash.mix(op.rawValue)
                layerHash.mix(hash)
                dump(op.dumpName, [.string(css)])
                guard let spec = decoder.fontSpec(css) else {
                    report("font \"\(css)\" has no size; the previous font was kept")
                    return
                }
                if let unknown = spec.unknownFamily {
                    report("font family \"\(unknown)\" is not available; using SF Pro "
                        + "(available: SF Pro, SF Pro Rounded, SF Mono, New York, system-ui)")
                }
                state.style.font = spec
            case .textAlign:
                guard let align = TextAlign(rawValue: Int(a[0])) else { return }
                state.style.textAlign = align
                mixRaw(op, a)
                dump(op.dumpName, [.string(align.cssName)])
            case .textBaseline:
                guard let baseline = TextBaseline(rawValue: Int(a[0])) else { return }
                state.style.textBaseline = baseline
                mixRaw(op, a)
                dump(op.dumpName, [.string(baseline.cssName)])

            // Layer breaks.
            case .fillGlass:
                fillGlass(a)
            case .fillBlur:
                fillBlur(a)
            case .vibrant:
                let on = a[0] != 0
                guard on != vibrant else { return }
                closeDrawLayer()
                vibrant = on
                layerHash = initialHash()

            case .frameInfo:
                list.recordedOps = a[0].isFinite ? Int(max(0, min(a[0], 1e9))) : 0
                list.droppedOps = a[1].isFinite ? Int(max(0, min(a[1], 1e9))) : 0
                list.hasFrameFunction = a[2] != 0
            }
        }

        mutating func mixRaw(_ op: OpCode, _ a: [Double]) {
            layerHash.mix(op.rawValue)
            for value in a { layerHash.mix(value) }
        }

        /// The path a paint op uses, in unit space, plus its content hash. Consumes a Path2D block.
        mutating func consumePath(usePath2D: Bool, allowEmpty: Bool = false) -> (CGPath, Hash64)? {
            if usePath2D {
                guard let ready = readyBlock else {
                    report("a paint op referenced a Path2D that was not recorded; the op was ignored")
                    return nil
                }
                readyBlock = nil
                guard allowEmpty || !ready.isEmpty else { return nil }
                var ctm = state.ctm
                let unit = ready.path.copy(using: &ctm) ?? ready.path
                var hash = ready.hash
                hash.mix(state.ctm)
                return (unit, hash)
            }
            guard allowEmpty || !path.isEmpty else { return nil }
            return (path.path.copy() ?? path.path, path.hash)
        }

        func blockDumpArgument(_ used: Bool) -> [JSONValue] {
            guard recordDump, used else { return [] }
            return [.object(["path2d": .array(readyBlockDump)])]
        }

        mutating func gradient(_ a: [Double], op: OpCode) -> GradientSpec? {
            guard a.count >= 8, let kind = GradientKind(rawValue: Int(a[0])) else {
                report("\(op.dumpName) has a malformed gradient; the op was ignored")
                return nil
            }
            let stopCount = Int(a[7])
            guard stopCount >= 0, a.count == 8 + stopCount * 2 else {
                report("\(op.dumpName) has a malformed gradient; the op was ignored")
                return nil
            }
            var stops: [GradientStop] = []
            for index in 0..<stopCount {
                let offset = a[8 + index * 2]
                guard let (css, _) = string(a[9 + index * 2], op: op) else { return nil }
                guard let color = decoder.color(css) else {
                    report("gradient color stop \"\(css)\" is not a CSS color; the stop was ignored")
                    continue
                }
                stops.append(GradientStop(offset: min(max(offset, 0), 1), color: color))
            }
            // Stable sort by offset (canvas keeps insertion order for equal offsets).
            let sorted = stops.enumerated().sorted { lhs, rhs in
                lhs.element.offset == rhs.element.offset ? lhs.offset < rhs.offset : lhs.element.offset < rhs.element.offset
            }.map(\.element)
            return GradientSpec(kind: kind, params: Array(a[1..<7]), stops: sorted)
        }

        func gradientDump(_ g: GradientSpec) -> JSONValue {
            let params: [Double] =
                switch g.kind {
                case .linear: Array(g.params.prefix(4))
                case .radial: g.params
                case .conic: Array(g.params.prefix(3))
                }
            return .object([
                "type": .string(g.kind.cssName),
                "params": .array(params.map(dumpNumber)),
                "stops": .array(g.stops.map { .array([dumpNumber($0.offset), .string($0.color.css)]) }),
            ])
        }

        mutating func drawImage(_ a: [Double]) {
            let id = Int(exactly: a[0]) ?? -1
            let declared = CGSize(width: a[1], height: a[2])
            var source = CGRect(x: a[3], y: a[4], width: a[5], height: a[6])
            var destination = CGRect(x: a[7], y: a[8], width: a[9], height: a[10])
            mixRaw(.drawImage, a)
            layerHash.mix(state.ctm)
            if let image = images[id] {
                layerHash.mix(UInt64(UInt(bitPattern: Unmanaged.passUnretained(image).toOpaque())))
            }
            dump("drawImage", [.object(["image": .int(Int64(id))])] + a[3...].map(dumpNumber))
            guard source.width != 0, source.height != 0, destination.width != 0, destination.height != 0,
                  declared.width > 0, declared.height > 0
            else { return }
            // Canvas normalises negative sizes by flipping the rectangle.
            source = source.standardized
            destination = destination.standardized
            // Clip the source to the image and shrink the destination proportionally.
            let bounds = CGRect(origin: .zero, size: declared)
            let clipped = source.intersection(bounds)
            guard !clipped.isNull, clipped.width > 0, clipped.height > 0 else { return }
            if clipped != source {
                let sx = destination.width / source.width, sy = destination.height / source.height
                destination = CGRect(x: destination.minX + (clipped.minX - source.minX) * sx,
                                     y: destination.minY + (clipped.minY - source.minY) * sy,
                                     width: clipped.width * sx, height: clipped.height * sy)
                source = clipped
            }
            add(.image(id: id, declaredSize: declared, source: source, destination: destination))
            if images[id] != nil { list.paintCount += 1 }
        }

        mutating func fillGlass(_ a: [Double]) {
            let style = GlassStyle(rawValue: Int(a[0])) ?? .regular
            let rule: CGPathFillRule = a[3] == 1 ? .evenOdd : .winding
            let usePath2D = a[4] == 1
            var tint: RGBA?
            var tintCSS: String?
            if a[1] >= 0, let (css, _) = string(a[1], op: .fillGlass) {
                tintCSS = css
                tint = decoder.color(css)
                if tint == nil { report("fillGlass tint \"\(css)\" is not a CSS color; the glass is untinted") }
            }
            let interactive = a[2] != 0

            let geometry: (path: CGPath, transform: CGAffineTransform, hash: Hash64)
            if usePath2D {
                guard let ready = readyBlock else {
                    report("fillGlass referenced a Path2D that was not recorded; the op was ignored")
                    return
                }
                readyBlock = nil
                geometry = (ready.path.copy() ?? ready.path, state.ctm, ready.hash)
            } else {
                geometry = path.glassGeometry()
            }
            guard !geometry.path.isEmpty else {
                report("fillGlass was called with an empty path; nothing was drawn")
                return
            }
            if list.glassLayerCount >= DrawingLimits.maxGlassFills {
                list.glassOverflow += 1
                report(OpDecoder.glassOverflowMessage)
                var t = geometry.transform
                let unit = geometry.path.copy(using: &t) ?? geometry.path
                layerHash.mix(OpCode.fillGlass.rawValue)
                layerHash.mix(geometry.hash)
                layerHash.mix(geometry.transform)
                add(.flatGlass(unit, rule, tint: tint))
                list.paintCount += 1
                let ruleName = rule == .evenOdd ? "evenodd" : "nonzero"
                dump("fillGlass", [.object(["flat": .bool(true), "rule": .string(ruleName)])])
                return
            }
            var outline = geometry.hash
            outline.mix(rule == .evenOdd)
            var options = Hash64(seed: 0x0971)
            options.mix(style.rawValue)
            options.mix(interactive)
            if let tint { tint.mix(into: &options) } else { options.mix(-1) }
            closeDrawLayer()
            list.layers.append(.glass(GlassLayer(
                localPath: geometry.path, rule: rule, style: style, tint: tint, tintCSS: tintCSS,
                interactive: interactive, transform: geometry.transform, outlineHash: outline, optionsHash: options)))
            list.paintCount += 1
            layerHash = initialHash()
        }

        mutating func fillBlur(_ a: [Double]) {
            let material = BlurMaterial(rawValue: Int(a[0])) ?? .hud
            let rule: CGPathFillRule = a[1] == 1 ? .evenOdd : .winding
            guard let (unit, pathHash) = consumePath(usePath2D: a[2] == 1) else {
                report("fillBlur was called with an empty path; nothing was drawn")
                return
            }
            if list.glassLayerCount >= DrawingLimits.maxGlassFills {
                list.glassOverflow += 1
                report(OpDecoder.glassOverflowMessage)
                layerHash.mix(OpCode.fillBlur.rawValue)
                layerHash.mix(pathHash)
                add(.flatGlass(unit, rule, tint: nil))
                list.paintCount += 1
                return
            }
            var hash = pathHash
            hash.mix(rule == .evenOdd)
            hash.mix(material.rawValue)
            closeDrawLayer()
            list.layers.append(.blur(BlurLayer(path: unit, rule: rule, material: material, hash: hash)))
            list.paintCount += 1
            layerHash = initialHash()
        }
    }
}
