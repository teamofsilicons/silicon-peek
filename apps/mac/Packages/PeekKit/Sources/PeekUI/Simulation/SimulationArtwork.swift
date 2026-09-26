import CoreGraphics
import CoreText
import Foundation
import ImageIO

/// Sample images for Simulation, drawn with Core Graphics: fictional album covers (the DJ case in
/// understanding.md) and option images for asks. Nothing is downloaded; every artist, title and
/// shape here is made up for Peek.
public enum SimulationArtwork {
    /// A fictional album cover (512 × 512).
    public enum Cover: String, CaseIterable, Sendable {
        case neonTide = "cover-neon-tide"
        case paperPlanes = "cover-paper-planes"
        case midnightStatic = "cover-midnight-static"

        public var title: String {
            switch self {
            case .neonTide: "Neon Tide"
            case .paperPlanes: "Paper Planes"
            case .midnightStatic: "Midnight Static"
            }
        }

        public var artist: String {
            switch self {
            case .neonTide: "Lumen Harbor"
            case .paperPlanes: "The Quiet Hours"
            case .midnightStatic: "Vela Park"
            }
        }

        public var year: String {
            switch self {
            case .neonTide: "2024"
            case .paperPlanes: "2019"
            case .midnightStatic: "2022"
            }
        }
    }

    /// An option image (256 × 256).
    public enum Icon: String, CaseIterable, Sendable {
        case keep = "option-keep"
        case archive = "option-archive"
        case delete = "option-delete"
        case calm = "option-calm"
        case upbeat = "option-upbeat"
        case dreamy = "option-dreamy"
        case dark = "option-dark"
    }

    public static let coverSize = 512
    public static let iconSize = 256

    public struct RenderError: Error, CustomStringConvertible, Sendable {
        public let description: String
    }

    public static func png(_ cover: Cover) throws(RenderError) -> Data {
        try render(size: coverSize, name: cover.rawValue) { context, size in drawCover(cover, in: context, size: size) }
    }

    public static func png(_ icon: Icon) throws(RenderError) -> Data {
        try render(size: iconSize, name: icon.rawValue) { context, size in drawIcon(icon, in: context, size: size) }
    }

    // MARK: Rendering

    private static func render(size: Int, name: String, draw: (CGContext, CGFloat) -> Void) throws(RenderError) -> Data {
        guard let space = CGColorSpace(name: CGColorSpace.sRGB),
            let context = CGContext(
                data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: 0, space: space,
                bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
        else { throw RenderError(description: "cannot create a \(size)×\(size) sRGB bitmap for \(name)") }
        context.setShouldAntialias(true)
        context.interpolationQuality = .high
        draw(context, CGFloat(size))
        guard let image = context.makeImage() else {
            throw RenderError(description: "cannot snapshot the bitmap for \(name)")
        }
        let data = NSMutableData()
        guard let destination = CGImageDestinationCreateWithData(data, "public.png" as CFString, 1, nil) else {
            throw RenderError(description: "ImageIO cannot create a PNG encoder for \(name)")
        }
        CGImageDestinationAddImage(destination, image, nil)
        guard CGImageDestinationFinalize(destination) else {
            throw RenderError(description: "ImageIO failed to encode \(name) as PNG")
        }
        return data as Data
    }

    private static func color(_ hex: UInt32, _ alpha: CGFloat = 1) -> CGColor {
        CGColor(
            srgbRed: CGFloat((hex >> 16) & 0xFF) / 255, green: CGFloat((hex >> 8) & 0xFF) / 255,
            blue: CGFloat(hex & 0xFF) / 255, alpha: alpha)
    }

    private static func linearGradient(_ context: CGContext, _ colors: [UInt32], from: CGPoint, to: CGPoint) {
        guard let gradient = CGGradient(
            colorsSpace: CGColorSpace(name: CGColorSpace.sRGB), colors: colors.map { color($0) } as CFArray, locations: nil)
        else { return }
        context.drawLinearGradient(gradient, start: from, end: to, options: [.drawsBeforeStartLocation, .drawsAfterEndLocation])
    }

    /// Font weights as Core Text weight traits (the same scale as `NSFont.Weight`).
    enum Weight: CGFloat {
        case light = -0.4
        case regular = 0
        case medium = 0.23
        case semibold = 0.3
        case bold = 0.4
        case heavy = 0.56
    }

    /// Draws `text` with its baseline's left end at `origin` (Core Graphics coordinates, y up).
    /// Pure Core Text, so it is safe off the main thread.
    private static func drawText(_ text: String, in context: CGContext, at origin: CGPoint, size: CGFloat,
                                 weight: Weight, color textColor: CGColor, tracking: CGFloat = 0) {
        let system = CTFontCreateUIFontForLanguage(.system, size, nil) ?? CTFontCreateWithName("Helvetica" as CFString, size, nil)
        let traits = [kCTFontWeightTrait: weight.rawValue] as CFDictionary
        let descriptor = CTFontDescriptorCreateCopyWithAttributes(
            CTFontCopyFontDescriptor(system), [kCTFontTraitsAttribute: traits] as CFDictionary)
        let font = CTFontCreateWithFontDescriptor(descriptor, size, nil)
        let attributes = [
            kCTFontAttributeName: font, kCTForegroundColorAttributeName: textColor, kCTKernAttributeName: tracking as CFNumber,
        ] as CFDictionary
        guard let string = CFAttributedStringCreate(nil, text as CFString, attributes) else { return }
        let line = CTLineCreateWithAttributedString(string)
        context.saveGState()
        context.textMatrix = .identity
        context.textPosition = origin
        CTLineDraw(line, context)
        context.restoreGState()
    }

    private static func drawCover(_ cover: Cover, in context: CGContext, size s: CGFloat) {
        let rect = CGRect(x: 0, y: 0, width: s, height: s)
        switch cover {
        case .neonTide:
            // Retro sunset: warm sky, a striped sun sinking into a navy sea.
            linearGradient(context, [0xFFD76A, 0xFF8A3D, 0xFF3C7D, 0x7A2BD1], from: CGPoint(x: 0, y: s), to: CGPoint(x: 0, y: s * 0.3))
            context.saveGState()
            let sun = CGRect(x: s * 0.22, y: s * 0.36, width: s * 0.56, height: s * 0.56)
            context.addEllipse(in: sun)
            context.clip()
            linearGradient(context, [0xFFF6B0, 0xFFB347], from: CGPoint(x: 0, y: sun.maxY), to: CGPoint(x: 0, y: sun.minY))
            context.setFillColor(color(0xFF3C7D, 0.9))
            for band in 0..<6 {
                let height = s * (0.012 + 0.006 * CGFloat(band))
                context.fill(CGRect(x: sun.minX, y: sun.minY + s * 0.03 + CGFloat(band) * s * 0.045, width: sun.width, height: height))
            }
            context.restoreGState()
            context.setFillColor(color(0x1B1F4B))
            context.fill(CGRect(x: 0, y: 0, width: s, height: s * 0.4))
            context.setStrokeColor(color(0xFF7AB6, 0.55))
            // Reflections on the water, kept above the title.
            for row in 0..<5 {
                let y = s * 0.37 - CGFloat(row * row) * s * 0.0075
                context.setLineWidth(s * 0.004 * CGFloat(row + 1))
                context.move(to: CGPoint(x: s * 0.016 * CGFloat(7 - row), y: y))
                context.addLine(to: CGPoint(x: s - s * 0.016 * CGFloat(7 - row), y: y))
                context.strokePath()
            }
            drawText(cover.title.uppercased(), in: context, at: CGPoint(x: s * 0.07, y: s * 0.13), size: s * 0.085,
                     weight: .heavy, color: color(0xFFFFFF), tracking: s * 0.004)
            drawText(cover.artist, in: context, at: CGPoint(x: s * 0.07, y: s * 0.07), size: s * 0.045,
                     weight: .medium, color: color(0xFFD6EA))
        case .paperPlanes:
            // Pastel sky with folded paper planes and dotted flight paths.
            linearGradient(context, [0xEAF4FF, 0xBFDFFF], from: CGPoint(x: 0, y: s), to: CGPoint(x: 0, y: 0))
            context.setStrokeColor(color(0x5A8FD6, 0.5))
            context.setLineWidth(s * 0.006)
            context.setLineDash(phase: 0, lengths: [s * 0.012, s * 0.02])
            for path in 0..<3 {
                let y = s * (0.3 + 0.2 * CGFloat(path))
                context.move(to: CGPoint(x: -s * 0.1, y: y - s * 0.1))
                context.addCurve(to: CGPoint(x: s * 1.1, y: y + s * 0.12),
                                 control1: CGPoint(x: s * 0.3, y: y + s * 0.2), control2: CGPoint(x: s * 0.6, y: y - s * 0.2))
                context.strokePath()
            }
            context.setLineDash(phase: 0, lengths: [])
            let planes: [(CGFloat, CGFloat, CGFloat, CGFloat)] = [
                (0.62, 0.68, 0.22, -0.25), (0.28, 0.46, 0.16, 0.2), (0.74, 0.3, 0.12, 0.45),
            ]
            for (x, y, scale, angle) in planes {
                context.saveGState()
                context.translateBy(x: s * x, y: s * y)
                context.rotate(by: angle)
                let w = s * scale
                context.setShadow(offset: CGSize(width: 0, height: -s * 0.01), blur: s * 0.02, color: color(0x1B3A66, 0.25))
                context.setFillColor(color(0xFFFFFF))
                context.move(to: CGPoint(x: w, y: 0))
                context.addLine(to: CGPoint(x: -w, y: w * 0.55))
                context.addLine(to: CGPoint(x: -w * 0.55, y: 0))
                context.closePath()
                context.fillPath()
                context.setFillColor(color(0xD7E8FB))
                context.move(to: CGPoint(x: w, y: 0))
                context.addLine(to: CGPoint(x: -w * 0.55, y: 0))
                context.addLine(to: CGPoint(x: -w * 0.85, y: -w * 0.4))
                context.closePath()
                context.fillPath()
                context.restoreGState()
            }
            drawText(cover.title, in: context, at: CGPoint(x: s * 0.07, y: s * 0.12), size: s * 0.08,
                     weight: .bold, color: color(0x1B3A66))
            drawText(cover.artist.uppercased(), in: context, at: CGPoint(x: s * 0.07, y: s * 0.065), size: s * 0.038,
                     weight: .semibold, color: color(0x3D6BA8), tracking: s * 0.006)
        case .midnightStatic:
            // Deep night with concentric rings and deterministic star noise.
            linearGradient(context, [0x1A1440, 0x0D1021, 0x05060F], from: CGPoint(x: 0, y: s), to: CGPoint(x: s, y: 0))
            var rng = SplitMix64(seed: 0x5EED_CAFE)
            for _ in 0..<220 {
                let x = rng.nextDouble() * Double(s), y = rng.nextDouble() * Double(s)
                let r = CGFloat(0.4 + rng.nextDouble() * 1.4) * s / 512
                context.setFillColor(color(0xFFFFFF, CGFloat(0.2 + rng.nextDouble() * 0.6)))
                context.fillEllipse(in: CGRect(x: x, y: y, width: Double(r * 2), height: Double(r * 2)))
            }
            let center = CGPoint(x: s * 0.62, y: s * 0.58)
            for ring in 0..<9 {
                let radius = s * (0.06 + 0.045 * CGFloat(ring))
                let hue: UInt32 = ring % 2 == 0 ? 0x3FE0D0 : 0x9D6BFF
                context.setStrokeColor(color(hue, 0.85 - 0.08 * CGFloat(ring)))
                context.setLineWidth(s * (0.012 - 0.001 * CGFloat(ring)))
                context.strokeEllipse(in: CGRect(x: center.x - radius, y: center.y - radius, width: radius * 2, height: radius * 2))
            }
            context.setFillColor(color(0x3FE0D0))
            context.fillEllipse(in: CGRect(x: center.x - s * 0.025, y: center.y - s * 0.025, width: s * 0.05, height: s * 0.05))
            drawText(cover.title.lowercased(), in: context, at: CGPoint(x: s * 0.07, y: s * 0.12), size: s * 0.075,
                     weight: .light, color: color(0xE8E6FF), tracking: s * 0.003)
            drawText(cover.artist, in: context, at: CGPoint(x: s * 0.07, y: s * 0.068), size: s * 0.04,
                     weight: .regular, color: color(0x3FE0D0))
        }
        context.setStrokeColor(color(0x000000, 0.12))
        context.setLineWidth(2)
        context.stroke(rect.insetBy(dx: 1, dy: 1))
    }

    private static func drawIcon(_ icon: Icon, in context: CGContext, size s: CGFloat) {
        // Full bleed: the chrome's image tile rounds and borders the corners itself, so transparent corners here
        // would show the backdrop between the tile's border and the artwork.
        context.saveGState()
        let colors: [UInt32]
        switch icon {
        case .keep: colors = [0x5EE08A, 0x28B463]
        case .archive: colors = [0x5AB0FF, 0x1A73E8]
        case .delete: colors = [0xFF7A6E, 0xE5352B]
        case .calm: colors = [0xA6E3F5, 0x4FA9D3]
        case .upbeat: colors = [0xFFD166, 0xFF8C42]
        case .dreamy: colors = [0xE0B3FF, 0x9B5DE5]
        case .dark: colors = [0x3A3A5C, 0x14142B]
        }
        linearGradient(context, colors, from: CGPoint(x: 0, y: s), to: CGPoint(x: s, y: 0))
        context.restoreGState()

        context.setStrokeColor(color(0xFFFFFF))
        context.setFillColor(color(0xFFFFFF))
        context.setLineCap(.round)
        context.setLineJoin(.round)
        context.setLineWidth(s * 0.07)
        switch icon {
        case .keep:
            context.move(to: CGPoint(x: s * 0.28, y: s * 0.52))
            context.addLine(to: CGPoint(x: s * 0.44, y: s * 0.35))
            context.addLine(to: CGPoint(x: s * 0.73, y: s * 0.68))
            context.strokePath()
        case .archive:
            context.setLineWidth(s * 0.055)
            context.stroke(CGRect(x: s * 0.26, y: s * 0.26, width: s * 0.48, height: s * 0.34))
            context.fill(CGRect(x: s * 0.22, y: s * 0.6, width: s * 0.56, height: s * 0.13))
            context.move(to: CGPoint(x: s * 0.42, y: s * 0.46))
            context.addLine(to: CGPoint(x: s * 0.58, y: s * 0.46))
            context.strokePath()
        case .delete:
            context.setLineWidth(s * 0.055)
            let can = CGMutablePath()
            can.move(to: CGPoint(x: s * 0.32, y: s * 0.66))
            can.addLine(to: CGPoint(x: s * 0.36, y: s * 0.24))
            can.addLine(to: CGPoint(x: s * 0.64, y: s * 0.24))
            can.addLine(to: CGPoint(x: s * 0.68, y: s * 0.66))
            context.addPath(can)
            context.strokePath()
            context.move(to: CGPoint(x: s * 0.26, y: s * 0.68))
            context.addLine(to: CGPoint(x: s * 0.74, y: s * 0.68))
            context.move(to: CGPoint(x: s * 0.43, y: s * 0.75))
            context.addLine(to: CGPoint(x: s * 0.57, y: s * 0.75))
            context.strokePath()
            context.setLineWidth(s * 0.04)
            for x in [0.44, 0.56] {
                context.move(to: CGPoint(x: s * x, y: s * 0.32))
                context.addLine(to: CGPoint(x: s * x, y: s * 0.58))
            }
            context.strokePath()
        case .calm:
            context.setLineWidth(s * 0.05)
            for row in 0..<3 {
                let y = s * (0.35 + 0.15 * CGFloat(row))
                context.move(to: CGPoint(x: s * 0.2, y: y))
                context.addCurve(to: CGPoint(x: s * 0.8, y: y), control1: CGPoint(x: s * 0.4, y: y + s * 0.1),
                                 control2: CGPoint(x: s * 0.6, y: y - s * 0.1))
            }
            context.strokePath()
        case .upbeat:
            context.setLineWidth(s * 0.06)
            context.move(to: CGPoint(x: s * 0.18, y: s * 0.4))
            for step in 1...6 {
                context.addLine(to: CGPoint(x: s * (0.18 + 0.107 * CGFloat(step)), y: s * (step % 2 == 0 ? 0.4 : 0.64)))
            }
            context.strokePath()
        case .dreamy:
            context.setFillColor(color(0xFFFFFF, 0.85))
            for (x, y, r) in [(0.4, 0.55, 0.16), (0.58, 0.6, 0.13), (0.52, 0.42, 0.12), (0.34, 0.4, 0.08)] {
                context.fillEllipse(in: CGRect(x: s * (x - r), y: s * (y - r), width: s * r * 2, height: s * r * 2))
            }
        case .dark:
            context.setFillColor(color(0xFFF3C4))
            context.fillEllipse(in: CGRect(x: s * 0.32, y: s * 0.32, width: s * 0.36, height: s * 0.36))
            context.setFillColor(color(0x14142B))
            context.fillEllipse(in: CGRect(x: s * 0.42, y: s * 0.38, width: s * 0.34, height: s * 0.34))
            context.setFillColor(color(0xFFFFFF, 0.9))
            for (x, y) in [(0.24, 0.72), (0.74, 0.76), (0.7, 0.24), (0.3, 0.26)] {
                context.fillEllipse(in: CGRect(x: s * x, y: s * y, width: s * 0.03, height: s * 0.03))
            }
        }
    }
}
