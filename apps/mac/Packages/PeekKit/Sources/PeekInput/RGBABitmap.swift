import CoreGraphics
import Foundation

/// An sRGB colour with components in 0…1 (gamma-encoded, as stored in images).
public struct SRGBColor: Sendable, Hashable, CustomStringConvertible {
    public var red: Double
    public var green: Double
    public var blue: Double

    public init(red: Double, green: Double, blue: Double) {
        self.red = red
        self.green = green
        self.blue = blue
    }

    public static let black = SRGBColor(red: 0, green: 0, blue: 0)

    /// `#rrggbb`, as drawings receive colours (visual.md A4).
    public var hex: String { PeekColorMath.hex(self) }
    public var description: String { hex }

    /// Linear interpolation: `t` 0 is `self`, 1 is `other`.
    public func mixed(with other: SRGBColor, amount t: Double) -> SRGBColor {
        SRGBColor(red: red + (other.red - red) * t, green: green + (other.green - green) * t,
                 blue: blue + (other.blue - blue) * t)
    }
}

enum PeekColorMath {
    static func hex(_ color: SRGBColor) -> String {
        func byte(_ c: Double) -> Int { Int((min(max(c.isFinite ? c : 0, 0), 1) * 255).rounded()) }
        return String(format: "#%02x%02x%02x", byte(color.red), byte(color.green), byte(color.blue))
    }
}

/// A small 8-bit RGBA copy of an image in sRGB (premultiplied alpha, top row first), for colour sampling:
/// backdrop averages (§8.8) and image palettes (visual.md B7).
public struct RGBABitmap: Sendable, Equatable {
    public let width: Int
    public let height: Int
    /// `width × height × 4` bytes: R, G, B (premultiplied), A.
    public let pixels: [UInt8]

    public init(width: Int, height: Int, pixels: [UInt8]) {
        precondition(width > 0 && height > 0 && pixels.count == width * height * 4,
                     "RGBABitmap needs width × height × 4 bytes")
        self.width = width
        self.height = height
        self.pixels = pixels
    }

    /// Renders `image` into an sRGB bitmap of `width × height` (default: the image's own size).
    public init?(image: CGImage, width: Int? = nil, height: Int? = nil) {
        let width = width ?? image.width
        let height = height ?? image.height
        guard width > 0, height > 0, let space = CGColorSpace(name: CGColorSpace.sRGB) else { return nil }
        var bytes = [UInt8](repeating: 0, count: width * height * 4)
        let drawn = bytes.withUnsafeMutableBytes { buffer -> Bool in
            guard let context = CGContext(
                data: buffer.baseAddress, width: width, height: height, bitsPerComponent: 8, bytesPerRow: width * 4,
                space: space,
                bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue | CGBitmapInfo.byteOrder32Big.rawValue)
            else { return false }
            context.interpolationQuality = .high
            context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
            return true
        }
        guard drawn else { return nil }
        self.init(width: width, height: height, pixels: bytes)
    }

    /// A bitmap filled with one opaque colour (tests, flat wallpapers).
    public init(width: Int, height: Int, fill color: SRGBColor) {
        var bytes = [UInt8](repeating: 255, count: width * height * 4)
        let components = [color.red, color.green, color.blue].map { UInt8((min(max($0, 0), 1) * 255).rounded()) }
        for pixel in 0..<(width * height) {
            bytes[pixel * 4] = components[0]
            bytes[pixel * 4 + 1] = components[1]
            bytes[pixel * 4 + 2] = components[2]
        }
        self.init(width: width, height: height, pixels: bytes)
    }

    /// The alpha-weighted average colour of the pixels overlapping `rect` (pixel units, y-down from the top-left),
    /// and the average opacity. Partly covered edge pixels count by their covered area. Nil when `rect` misses
    /// the bitmap or covers only transparent pixels.
    public func averageColor(in rect: CGRect) -> (color: SRGBColor, opacity: Double)? {
        let clipped = rect.standardized.intersection(CGRect(x: 0, y: 0, width: width, height: height))
        guard !clipped.isNull, clipped.width > 0, clipped.height > 0 else { return nil }
        let x0 = Int(clipped.minX.rounded(.down)), x1 = Int(clipped.maxX.rounded(.up))
        let y0 = Int(clipped.minY.rounded(.down)), y1 = Int(clipped.maxY.rounded(.up))
        var red = 0.0, green = 0.0, blue = 0.0, alpha = 0.0, area = 0.0
        for y in y0..<y1 {
            let coverY = Double(min(clipped.maxY, CGFloat(y + 1)) - max(clipped.minY, CGFloat(y)))
            guard coverY > 0 else { continue }
            for x in x0..<x1 {
                let coverX = Double(min(clipped.maxX, CGFloat(x + 1)) - max(clipped.minX, CGFloat(x)))
                guard coverX > 0 else { continue }
                let weight = coverX * coverY
                let offset = (y * width + x) * 4
                red += Double(pixels[offset]) * weight
                green += Double(pixels[offset + 1]) * weight
                blue += Double(pixels[offset + 2]) * weight
                alpha += Double(pixels[offset + 3]) * weight
                area += weight
            }
        }
        guard area > 0, alpha > 0 else { return nil }
        // Premultiplied sums divided by the alpha sum give the alpha-weighted mean colour.
        return (SRGBColor(red: red / alpha, green: green / alpha, blue: blue / alpha), alpha / (area * 255))
    }

    /// The colour of one pixel (un-premultiplied) and its alpha, 0…1.
    public func pixel(x: Int, y: Int) -> (color: SRGBColor, alpha: Double) {
        let offset = (y * width + x) * 4
        let alpha = Double(pixels[offset + 3]) / 255
        guard alpha > 0 else { return (.black, 0) }
        return (SRGBColor(red: Double(pixels[offset]) / 255 / alpha, green: Double(pixels[offset + 1]) / 255 / alpha,
                         blue: Double(pixels[offset + 2]) / 255 / alpha), alpha)
    }
}
