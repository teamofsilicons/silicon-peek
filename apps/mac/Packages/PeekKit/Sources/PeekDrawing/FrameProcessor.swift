import CoreGraphics
import Foundation

/// A frame ready for the compositor: glass and blur specs plus a bitmap per draw layer.
enum RenderedLayer: @unchecked Sendable {
    case draw(hash: Hash64, image: CGImage, vibrant: Bool)
    case glass(GlassLayer)
    case blur(BlurLayer)

    var kindName: String {
        switch self {
        case .draw(_, _, let vibrant): vibrant ? "vibrant-draw" : "draw"
        case .glass: "glass"
        case .blur: "blur"
        }
    }
}

struct RenderedFrame: @unchecked Sendable {
    var layers: [RenderedLayer]
    var again: Bool
    var diagnostics: [String]
    var recordedOps: Int
    var droppedOps: Int
    var glassOverflow: Int
}

/// Decodes and rasterises frames for one drawing (visual.md B5 split → diff → render, the render half of
/// the draw layers). It lives on the drawing's ``JSThread`` next to the VM, so the main thread only swaps
/// layer contents. Draw layers whose hash did not change reuse last frame's bitmap.
final class FrameProcessor {
    let decoder: OpDecoder
    let colorSpace: CGColorSpace
    private var cache: [UInt64: CGImage] = [:]

    init(decoder: OpDecoder = OpDecoder(), colorSpace: CGColorSpace = CGReplay.displayP3) {
        self.decoder = decoder
        self.colorSpace = colorSpace
    }

    func process(ops: [Double], strings: [String], again: Bool, images: [Int: CGImage], pixels: Int) -> RenderedFrame {
        let list = decoder.decode(ops: ops, strings: strings, again: again, images: images)
        return render(list, images: images, pixels: pixels)
    }

    func render(_ list: DisplayList, images: [Int: CGImage], pixels: Int) -> RenderedFrame {
        var next: [UInt64: CGImage] = [:]
        var layers: [RenderedLayer] = []
        layers.reserveCapacity(list.layers.count)
        for layer in list.layers {
            switch layer {
            case .draw(let draw):
                var key = draw.hash
                key.mix(pixels)
                key.mix(draw.vibrant)
                let image: CGImage
                if let cached = cache[key.value] ?? next[key.value] {
                    image = cached
                } else {
                    let environment = ReplayEnvironment(images: images, monochrome: draw.vibrant)
                    guard let rendered = CGReplay.render(draw.commands, pixels: pixels, environment: environment,
                                                         colorSpace: colorSpace)
                    else { continue }
                    image = rendered
                }
                next[key.value] = image
                layers.append(.draw(hash: draw.hash, image: image, vibrant: draw.vibrant))
            case .glass(let glass):
                layers.append(.glass(glass))
            case .blur(let blur):
                layers.append(.blur(blur))
            }
        }
        cache = next
        return RenderedFrame(layers: layers, again: list.again, diagnostics: list.diagnostics,
                             recordedOps: list.recordedOps, droppedOps: list.droppedOps,
                             glassOverflow: list.glassOverflow)
    }

    func reset() { cache.removeAll() }
}

/// Serialises a `CGPath` as SVG path data (`M`, `L`, `Q`, `C`, `Z`) with at most 3 decimals.
enum SVGPathWriter {
    static func string(_ path: CGPath) -> String {
        var parts: [String] = []
        path.applyWithBlock { pointer in
            let element = pointer.pointee
            let p = element.points
            switch element.type {
            case .moveToPoint: parts.append("M\(n(p[0].x)) \(n(p[0].y))")
            case .addLineToPoint: parts.append("L\(n(p[0].x)) \(n(p[0].y))")
            case .addQuadCurveToPoint: parts.append("Q\(n(p[0].x)) \(n(p[0].y)) \(n(p[1].x)) \(n(p[1].y))")
            case .addCurveToPoint:
                parts.append("C\(n(p[0].x)) \(n(p[0].y)) \(n(p[1].x)) \(n(p[1].y)) \(n(p[2].x)) \(n(p[2].y))")
            case .closeSubpath: parts.append("Z")
            @unknown default: break
            }
        }
        return parts.joined(separator: " ")
    }

    private static func n(_ value: CGFloat) -> String {
        let rounded = (Double(value) * 1000).rounded() / 1000
        if rounded == rounded.rounded(), abs(rounded) < 1e15 { return String(Int64(rounded)) }
        var text = String(format: "%.3f", rounded)
        while text.hasSuffix("0") { text.removeLast() }
        if text.hasSuffix(".") { text.removeLast() }
        return text == "-0" ? "0" : text
    }
}
