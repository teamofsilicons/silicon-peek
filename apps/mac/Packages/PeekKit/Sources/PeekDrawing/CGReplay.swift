import CoreGraphics
import CoreImage
import CoreText
import Foundation

/// What a replay needs besides the commands.
struct ReplayEnvironment {
    /// Live image handles of this frame (visual.md B7): drawing an unknown handle does nothing.
    var images: [Int: CGImage] = [:]
    var fonts: FontCache = .shared
    /// Vibrant layers are drawn in grey so the system vibrancy can colour them (visual.md B5).
    var monochrome = false
    /// Glass/blur fills drawn as a flat translucent approximation (validation `--preview`).
    var approximateGlass = false
}

/// Replays resolved ``DrawCommand``s into a `CGContext` (visual.md B6).
enum CGReplay {
    static let ciContext = CIContext(options: [.cacheIntermediates: false])
    static let sRGB = CGColorSpace(name: CGColorSpace.sRGB)!
    static let displayP3 = CGColorSpace(name: CGColorSpace.displayP3)!

    /// A transparent square bitmap context with the unit → pixel, y-down transform installed.
    static func makeContext(pixels: Int, colorSpace: CGColorSpace = displayP3) -> CGContext? {
        guard pixels > 0,
              let context = CGContext(
                data: nil, width: pixels, height: pixels, bitsPerComponent: 8, bytesPerRow: 0, space: colorSpace,
                bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue)
        else { return nil }
        prepare(context, pixels: pixels)
        return context
    }

    /// Installs y-down and 100 units → `pixels`.
    static func prepare(_ context: CGContext, pixels: Int) {
        context.translateBy(x: 0, y: CGFloat(pixels))
        context.scaleBy(x: 1, y: -1)
        context.scaleBy(x: CGFloat(pixels) / 100, y: CGFloat(pixels) / 100)
        context.setShouldSmoothFonts(false)
        context.setShouldSubpixelQuantizeFonts(false)
        context.interpolationQuality = .high
    }

    /// Renders one layer into a new image of `pixels` × `pixels`.
    static func render(_ commands: [DrawCommand], pixels: Int, environment: ReplayEnvironment,
                       colorSpace: CGColorSpace = displayP3) -> CGImage? {
        guard let context = makeContext(pixels: pixels, colorSpace: colorSpace) else { return nil }
        replay(commands, in: context, pixels: pixels, environment: environment)
        return context.makeImage()
    }

    /// Replays commands into a context prepared with ``prepare(_:pixels:)``.
    static func replay(_ commands: [DrawCommand], in context: CGContext, pixels: Int, environment: ReplayEnvironment) {
        let scale = CGFloat(pixels) / 100
        var appliedClip: ClipNode?
        context.saveGState()
        for command in commands {
            if command.clip !== appliedClip {
                context.restoreGState()
                context.saveGState()
                if let clip = command.clip {
                    for node in clip.chain {
                        context.addPath(node.path)
                        context.clip(using: node.rule)
                    }
                }
                appliedClip = command.clip
            }
            var style = command.style
            if environment.monochrome {
                style.fill = style.fill.monochrome
                style.stroke = style.stroke.monochrome
                style.shadowColor = style.shadowColor.monochrome
            }
            if style.filterBlur > 0, !isClear(command.kind) {
                drawBlurred(command, style: style, in: context, pixels: pixels, scale: scale, environment: environment)
            } else {
                draw(command, style: style, in: context, scale: scale, environment: environment)
            }
        }
        context.restoreGState()
    }

    private static func isClear(_ kind: DrawCommand.Kind) -> Bool {
        if case .clear = kind { return true }
        return false
    }

    static func blendMode(_ mode: CompositeMode) -> CGBlendMode {
        switch mode {
        case .sourceOver: .normal
        case .multiply: .multiply
        case .screen: .screen
        case .overlay: .overlay
        case .destinationOut: .destinationOut
        case .lighter: .plusLighter
        }
    }

    private static func draw(_ command: DrawCommand, style: Style, in context: CGContext, scale: CGFloat,
                             environment: ReplayEnvironment) {
        context.saveGState()
        defer { context.restoreGState() }
        if case .clear(let path) = command.kind {
            // clearRect ignores alpha, compositing, shadows and filters, but respects the clip.
            context.setBlendMode(.clear)
            context.addPath(path)
            context.fillPath()
            return
        }
        context.setAlpha(style.globalAlpha)
        context.setBlendMode(blendMode(style.composite))
        let shadow = style.hasShadow
        if shadow {
            // Shadows ignore the transform (canvas and CG agree); CG's base space is y-up pixels.
            context.setShadow(offset: CGSize(width: style.shadowOffsetX * scale, height: -style.shadowOffsetY * scale),
                              blur: style.shadowBlur * scale, color: style.shadowColor.cgColor)
            context.beginTransparencyLayer(auxiliaryInfo: nil)
        }
        drawContent(command, style: style, in: context, environment: environment)
        if shadow { context.endTransparencyLayer() }
    }

    /// `filter = 'blur(Npx)'`: render the op alone, blur it with CIGaussianBlur, composite it back.
    private static func drawBlurred(_ command: DrawCommand, style: Style, in context: CGContext, pixels: Int,
                                    scale: CGFloat, environment: ReplayEnvironment) {
        guard let offscreen = makeContext(pixels: pixels, colorSpace: context.colorSpace ?? displayP3) else { return }
        var plain = style
        plain.globalAlpha = 1
        plain.composite = .sourceOver
        plain.shadowColor = .transparent
        plain.filterBlur = 0
        var unclipped = command
        unclipped.clip = nil
        draw(unclipped, style: plain, in: offscreen, scale: scale, environment: environment)
        guard let sharp = offscreen.makeImage() else { return }
        let input = CIImage(cgImage: sharp)
        let blurred = input.applyingGaussianBlur(sigma: style.filterBlur * Double(scale)).cropped(to: input.extent)
        guard let image = ciContext.createCGImage(blurred, from: input.extent) else { return }
        context.saveGState()
        defer { context.restoreGState() }
        context.setAlpha(style.globalAlpha)
        context.setBlendMode(blendMode(style.composite))
        if style.hasShadow {
            context.setShadow(offset: CGSize(width: style.shadowOffsetX * scale, height: -style.shadowOffsetY * scale),
                              blur: style.shadowBlur * scale, color: style.shadowColor.cgColor)
        }
        context.translateBy(x: 0, y: 100)
        context.scaleBy(x: 1, y: -1)
        context.draw(image, in: CGRect(x: 0, y: 0, width: 100, height: 100))
    }

    // swiftlint:disable:next function_body_length
    private static func drawContent(_ command: DrawCommand, style: Style, in context: CGContext,
                                    environment: ReplayEnvironment) {
        switch command.kind {
        case .fill(let path, let rule):
            paint(style.fill, context: context, ctm: command.ctm) {
                context.addPath(path)
                return rule
            }
        case .stroke(let unitPath):
            let ctm = command.ctm
            guard ctm.a * ctm.d - ctm.b * ctm.c != 0 else { return }
            var inverse = ctm.inverted()
            guard let local = unitPath.copy(using: &inverse) else { return }
            context.concatenate(ctm)
            applyLineStyle(style, to: context)
            switch style.stroke {
            case .color(let color):
                context.setStrokeColor(color.cgColor)
                context.addPath(local)
                context.strokePath()
            case .gradient(let gradient):
                context.addPath(local)
                context.replacePathWithStrokedPath()
                guard !context.isPathEmpty else { return }
                context.clip()
                drawGradient(gradient, in: context)
            }
        case .clear:
            break
        case .text(let text, let x, let y, let maxWidth, let isStroke):
            drawText(text, x: x, y: y, maxWidth: maxWidth, stroke: isStroke, style: style, ctm: command.ctm,
                     context: context, fonts: environment.fonts)
        case .image(let id, let declared, let source, let destination):
            guard let image = environment.images[id] else { return }
            let kx = destination.width / source.width, ky = destination.height / source.height
            let full = CGRect(x: destination.minX - source.minX * kx, y: destination.minY - source.minY * ky,
                              width: declared.width * kx, height: declared.height * ky)
            context.concatenate(command.ctm)
            context.clip(to: destination)
            context.translateBy(x: full.minX, y: full.maxY)
            context.scaleBy(x: 1, y: -1)
            context.draw(image, in: CGRect(x: 0, y: 0, width: full.width, height: full.height))
        case .flatGlass(let path, let rule, let tint):
            drawFlatGlass(path, rule: rule, tint: tint, in: context)
        }
    }

    /// The flat translucent stand-in for glass (A7 overflow and the `--preview` grid).
    static func drawFlatGlass(_ path: CGPath, rule: CGPathFillRule, tint: RGBA?, in context: CGContext) {
        context.saveGState()
        defer { context.restoreGState() }
        // A light frost, then the tint at its own opacity (capped so the glass never turns opaque).
        context.setFillColor(RGBA.white.withAlpha(0.28).cgColor)
        context.addPath(path)
        context.fillPath(using: rule)
        if let tint, tint.alpha > 0 {
            context.setFillColor(tint.withAlpha(min(tint.alpha, 0.5)).cgColor)
            context.addPath(path)
            context.fillPath(using: rule)
        }
        context.setStrokeColor(RGBA.white.withAlpha(0.55).cgColor)
        context.setLineWidth(0.6)
        context.addPath(path)
        context.strokePath()
    }

    static func drawFlatBlur(_ path: CGPath, rule: CGPathFillRule, in context: CGContext) {
        context.saveGState()
        defer { context.restoreGState() }
        context.setFillColor(RGBA(red: 0.55, green: 0.55, blue: 0.58, alpha: 0.55).cgColor)
        context.addPath(path)
        context.fillPath(using: rule)
    }

    private static func applyLineStyle(_ style: Style, to context: CGContext) {
        context.setLineWidth(style.lineWidth)
        context.setLineCap(style.lineCap == .round ? .round : style.lineCap == .square ? .square : .butt)
        context.setLineJoin(style.lineJoin == .round ? .round : style.lineJoin == .bevel ? .bevel : .miter)
        context.setMiterLimit(style.miterLimit)
        if !style.lineDash.isEmpty, style.lineDash.contains(where: { $0 > 0 }) {
            context.setLineDash(phase: style.lineDashOffset, lengths: style.lineDash.map { CGFloat($0) })
        }
    }

    /// Fills the path added by `addPath` with a colour, or clips to it and draws a gradient in `ctm` space.
    private static func paint(_ paint: Paint, context: CGContext, ctm: CGAffineTransform,
                              addPath: () -> CGPathFillRule) {
        switch paint {
        case .color(let color):
            context.setFillColor(color.cgColor)
            let rule = addPath()
            context.fillPath(using: rule)
        case .gradient(let gradient):
            let rule = addPath()
            guard !context.isPathEmpty else { return }
            context.clip(using: rule)
            context.concatenate(ctm)
            drawGradient(gradient, in: context)
        }
    }

    /// Draws a gradient over the whole clip, in the current user space.
    static func drawGradient(_ spec: GradientSpec, in context: CGContext) {
        guard !spec.stops.isEmpty else { return }
        if spec.stops.count == 1 {
            context.setFillColor(spec.stops[0].color.cgColor)
            context.fill(context.boundingBoxOfClipPath)
            return
        }
        let colors = spec.stops.map(\.color.cgColor) as CFArray
        let locations = spec.stops.map { CGFloat($0.offset) }
        guard let gradient = CGGradient(colorsSpace: sRGB, colors: colors, locations: locations) else { return }
        let p = spec.params
        let extend: CGGradientDrawingOptions = [.drawsBeforeStartLocation, .drawsAfterEndLocation]
        switch spec.kind {
        case .linear:
            guard p[0] != p[2] || p[1] != p[3] else { return }
            context.drawLinearGradient(gradient, start: CGPoint(x: p[0], y: p[1]), end: CGPoint(x: p[2], y: p[3]),
                                       options: extend)
        case .radial:
            guard p[0] != p[3] || p[1] != p[4] || p[2] != p[5] else { return }
            context.drawRadialGradient(gradient, startCenter: CGPoint(x: p[0], y: p[1]), startRadius: p[2],
                                       endCenter: CGPoint(x: p[3], y: p[4]), endRadius: p[5], options: extend)
        case .conic:
            // Canvas conic gradients run clockwise on screen from `startAngle`; CG's run toward increasing
            // angles in user space, which is also clockwise in this y-down space.
            CGContextDrawConicGradient(context, gradient, CGPoint(x: p[1], y: p[2]), p[0])
        }
    }

    // swiftlint:disable:next function_parameter_count
    private static func drawText(_ text: String, x: Double, y: Double, maxWidth: Double?, stroke: Bool, style: Style,
                                 ctm: CGAffineTransform, context: CGContext, fonts: FontCache) {
        guard !text.isEmpty else { return }
        let font = fonts.font(for: style.font)
        let line = fonts.line(text, font: font)
        let width = CTLineGetTypographicBounds(line, nil, nil, nil)
        guard width > 0 else { return }
        let squeeze = maxWidth.map { min(1, $0 / width) } ?? 1
        let drawnWidth = width * squeeze
        let alignOffset: Double =
            switch style.textAlign {
            case .start, .left: 0
            case .end, .right: -drawnWidth
            case .center: -drawnWidth / 2
            }
        let ascent = Double(CTFontGetAscent(font)), descent = Double(CTFontGetDescent(font))
        let baselineOffset: Double =
            switch style.textBaseline {
            case .alphabetic: 0
            case .top: ascent
            case .hanging: ascent * 0.8
            case .middle: (ascent - descent) / 2
            case .ideographic, .bottom: -descent
            }
        let textTransform = CGAffineTransform(translationX: x + alignOffset, y: y + baselineOffset)
            .scaledBy(x: squeeze, y: -1)
        context.concatenate(ctm)
        context.concatenate(textTransform)
        context.textPosition = .zero
        let paint = stroke ? style.stroke : style.fill
        switch paint {
        case .color(let color):
            if stroke {
                context.setStrokeColor(color.cgColor)
                context.setLineWidth(style.lineWidth)
                context.setLineJoin(style.lineJoin == .round ? .round : style.lineJoin == .bevel ? .bevel : .miter)
                context.setTextDrawingMode(.stroke)
            } else {
                context.setFillColor(color.cgColor)
                context.setTextDrawingMode(.fill)
            }
            CTLineDraw(line, context)
        case .gradient(let gradient):
            if stroke {
                context.setLineWidth(style.lineWidth)
                context.setTextDrawingMode(.strokeClip)
            } else {
                context.setTextDrawingMode(.clip)
            }
            CTLineDraw(line, context)
            context.concatenate(textTransform.inverted())
            drawGradient(gradient, in: context)
        }
    }
}
