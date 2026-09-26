// Renders Peek's app icon (a glass orb over a dusk squircle, with the information
// arc beneath it) into an asset-catalog AppIcon.appiconset.
//
// Usage: xcrun swift apps/mac/scripts/make-app-icon.swift apps/mac/Resources/Assets.xcassets/AppIcon.appiconset
//
// Pure CoreGraphics + ImageIO, deterministic, no network. Re-run it after changing the design
// and commit the PNGs; the build does not run this script.
import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

let arguments = CommandLine.arguments
guard arguments.count == 2 else {
    FileHandle.standardError.write(Data("usage: make-app-icon.swift <AppIcon.appiconset directory>\n".utf8))
    exit(2)
}
let outputDirectory = URL(fileURLWithPath: arguments[1], isDirectory: true)
try FileManager.default.createDirectory(at: outputDirectory, withIntermediateDirectories: true)

let space = CGColorSpace(name: CGColorSpace.displayP3)!

func color(_ r: CGFloat, _ g: CGFloat, _ b: CGFloat, _ a: CGFloat = 1) -> CGColor {
    CGColor(colorSpace: space, components: [r, g, b, a])!
}

func gradient(_ stops: [(CGFloat, CGColor)]) -> CGGradient {
    CGGradient(colorsSpace: space, colors: stops.map(\.1) as CFArray, locations: stops.map(\.0))!
}

/// Draws the 1024-point master in a y-up context scaled to `pixels`.
func render(pixels: Int) -> CGImage {
    let context = CGContext(data: nil, width: pixels, height: pixels, bitsPerComponent: 8, bytesPerRow: 0, space: space,
                            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    let scale = CGFloat(pixels) / 1024
    context.scaleBy(x: scale, y: scale)
    context.interpolationQuality = .high
    context.setShouldAntialias(true)

    // macOS icon grid: 824 × 824 body centred in 1024, continuous-looking corners.
    let body = CGRect(x: 100, y: 100, width: 824, height: 824)
    let squircle = CGPath(roundedRect: body, cornerWidth: 186, cornerHeight: 186, transform: nil)

    // Soft drop shadow under the body.
    context.saveGState()
    context.setShadow(offset: CGSize(width: 0, height: -12), blur: 28, color: color(0, 0, 0, 0.35))
    context.addPath(squircle)
    context.setFillColor(color(0.10, 0.12, 0.22))
    context.fillPath()
    context.restoreGState()

    // Dusk background (the reference wallpaper's palette: navy sky into warm horizon).
    context.saveGState()
    context.addPath(squircle)
    context.clip()
    context.drawLinearGradient(
        gradient([(0, color(0.95, 0.62, 0.40)), (0.38, color(0.55, 0.40, 0.58)), (1, color(0.12, 0.16, 0.36))]),
        start: CGPoint(x: 512, y: 100), end: CGPoint(x: 512, y: 924), options: [])
    // A faint glow behind the orb.
    context.drawRadialGradient(
        gradient([(0, color(1, 1, 1, 0.28)), (1, color(1, 1, 1, 0))]),
        startCenter: CGPoint(x: 512, y: 590), startRadius: 0, endCenter: CGPoint(x: 512, y: 590), endRadius: 380,
        options: [])
    context.restoreGState()

    // The information arc under the orb (peek all-positions.jpg: a wide arc facing the screen centre).
    context.saveGState()
    let arcCenter = CGPoint(x: 512, y: 990)
    context.addArc(center: arcCenter, radius: 700, startAngle: -.pi / 2 - 0.36, endAngle: -.pi / 2 + 0.36, clockwise: false)
    context.setLineWidth(26)
    context.setLineCap(.round)
    context.setStrokeColor(color(1, 1, 1, 0.78))
    context.strokePath()
    context.restoreGState()

    // Glass orb.
    let orbCenter = CGPoint(x: 512, y: 590)
    let orbRadius: CGFloat = 230
    let orb = CGPath(ellipseIn: CGRect(x: orbCenter.x - orbRadius, y: orbCenter.y - orbRadius,
                                       width: orbRadius * 2, height: orbRadius * 2), transform: nil)
    context.saveGState()
    context.setShadow(offset: CGSize(width: 0, height: -18), blur: 40, color: color(0.05, 0.05, 0.15, 0.45))
    context.addPath(orb)
    context.setFillColor(color(1, 1, 1, 0.16))
    context.fillPath()
    context.restoreGState()

    context.saveGState()
    context.addPath(orb)
    context.clip()
    // Body: bright upper-left, clearer lower-right, like refracted light.
    context.drawRadialGradient(
        gradient([(0, color(1, 1, 1, 0.55)), (0.55, color(0.85, 0.90, 1, 0.20)), (1, color(0.70, 0.80, 1, 0.10))]),
        startCenter: CGPoint(x: orbCenter.x - 80, y: orbCenter.y + 90), startRadius: 10,
        endCenter: orbCenter, endRadius: orbRadius, options: [.drawsBeforeStartLocation, .drawsAfterEndLocation])
    // Caustic at the bottom.
    context.drawRadialGradient(
        gradient([(0, color(1, 0.93, 0.85, 0.55)), (1, color(1, 0.93, 0.85, 0))]),
        startCenter: CGPoint(x: orbCenter.x + 30, y: orbCenter.y - 170), startRadius: 0,
        endCenter: CGPoint(x: orbCenter.x + 30, y: orbCenter.y - 170), endRadius: 150, options: [])
    // Specular highlight.
    context.saveGState()
    context.translateBy(x: orbCenter.x - 70, y: orbCenter.y + 120)
    context.rotate(by: 0.5)
    context.addEllipse(in: CGRect(x: -95, y: -45, width: 190, height: 90))
    context.clip()
    context.drawLinearGradient(
        gradient([(0, color(1, 1, 1, 0.95)), (1, color(1, 1, 1, 0.05))]),
        start: CGPoint(x: 0, y: 45), end: CGPoint(x: 0, y: -45), options: [])
    context.restoreGState()
    context.restoreGState()

    // Rim light.
    context.saveGState()
    context.addPath(orb)
    context.setLineWidth(7)
    context.setStrokeColor(color(1, 1, 1, 0.75))
    context.strokePath()
    context.restoreGState()

    return context.makeImage()!
}

func writePNG(_ image: CGImage, to url: URL) throws {
    guard let destination = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil) else {
        throw CocoaError(.fileWriteUnknown, userInfo: [NSFilePathErrorKey: url.path])
    }
    CGImageDestinationAddImage(destination, image, nil)
    guard CGImageDestinationFinalize(destination) else {
        throw CocoaError(.fileWriteUnknown, userInfo: [NSFilePathErrorKey: url.path])
    }
}

// Every macOS AppIcon slot: (point size, scale).
let slots: [(Int, Int)] = [(16, 1), (16, 2), (32, 1), (32, 2), (128, 1), (128, 2), (256, 1), (256, 2), (512, 1), (512, 2)]
var images: [[String: String]] = []
var rendered: [Int: String] = [:]
for (size, scale) in slots {
    let pixels = size * scale
    let filename = "icon_\(pixels).png"
    if rendered[pixels] == nil {
        try writePNG(render(pixels: pixels), to: outputDirectory.appendingPathComponent(filename))
        rendered[pixels] = filename
    }
    images.append(["idiom": "mac", "size": "\(size)x\(size)", "scale": "\(scale)x", "filename": filename])
}
let contents: [String: Any] = ["images": images, "info": ["author": "xcode", "version": 1]]
let json = try JSONSerialization.data(withJSONObject: contents, options: [.prettyPrinted, .sortedKeys])
try json.write(to: outputDirectory.appendingPathComponent("Contents.json"))
print("wrote \(rendered.count) PNGs and Contents.json to \(outputDirectory.path)")
