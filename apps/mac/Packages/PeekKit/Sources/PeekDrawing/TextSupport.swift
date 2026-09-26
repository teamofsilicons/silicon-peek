import AppKit
import CoreText
import Foundation

/// The system font families drawings may use (visual.md A5).
enum FontFamily: String, Hashable, Sendable {
    case system = "SF Pro"
    case rounded = "SF Pro Rounded"
    case mono = "SF Mono"
    case serif = "New York"
}

/// A parsed CSS font shorthand, e.g. `600 6px SF Pro` or `italic bold 8px "New York", serif`.
struct FontSpec: Hashable, Sendable {
    var family: FontFamily
    /// CSS weight 1…1000.
    var weight: Double
    var italic: Bool
    /// Size in drawing units (`8px` = 8 units).
    var size: Double
    /// A family name that was not recognised (the font falls back to SF Pro).
    var unknownFamily: String?

    static let `default` = FontSpec(family: .system, weight: 400, italic: false, size: 10, unknownFamily: nil)

    /// Parses `[style] [variant] [weight] [stretch] size[/line-height] family[, family…]`. Returns `nil`
    /// when there is no size (canvas then keeps the previous font).
    static func parse(_ css: String) -> FontSpec? {
        let tokens = css.split(whereSeparator: { $0 == " " || $0 == "\t" }).map(String.init)
        guard let sizeIndex = tokens.firstIndex(where: { sizeValue($0) != nil }), let size = sizeValue(tokens[sizeIndex])
        else { return nil }
        var spec = FontSpec.default
        spec.size = size
        for token in tokens[..<sizeIndex] {
            switch token.lowercased() {
            case "italic", "oblique": spec.italic = true
            case "bold", "bolder": spec.weight = 700
            case "lighter": spec.weight = 300
            case "normal", "small-caps", "condensed", "expanded", "semi-condensed", "semi-expanded": break
            default:
                if let value = Double(token), (1...1000).contains(value) { spec.weight = value }
            }
        }
        let familyText = tokens[(sizeIndex + 1)...].joined(separator: " ")
        let families = familyText.split(separator: ",").map {
            $0.trimmingCharacters(in: .whitespaces).trimmingCharacters(in: CharacterSet(charactersIn: "\"'"))
        }.filter { !$0.isEmpty }
        if let match = families.lazy.compactMap(family(named:)).first {
            spec.family = match
        } else if let first = families.first {
            spec.unknownFamily = first
        }
        return spec
    }

    private static func sizeValue(_ token: String) -> Double? {
        let head = token.split(separator: "/", maxSplits: 1).first.map(String.init) ?? token
        let lower = head.lowercased()
        let factor: Double
        let digits: Substring
        if lower.hasSuffix("px") {
            factor = 1
            digits = lower.dropLast(2)
        } else if lower.hasSuffix("pt") {
            factor = 4.0 / 3.0
            digits = lower.dropLast(2)
        } else {
            return nil
        }
        guard let value = Double(digits), value.isFinite, value > 0 else { return nil }
        return value * factor
    }

    private static func family(named name: String) -> FontFamily? {
        switch name.lowercased() {
        case "sf pro", "sf pro text", "sf pro display", "system-ui", "-apple-system", "blinkmacsystemfont", "sans-serif",
             "ui-sans-serif", "helvetica", "helvetica neue", "arial":
            .system
        case "sf pro rounded", "ui-rounded":
            .rounded
        case "sf mono", "ui-monospace", "monospace", "menlo", "monaco", "courier", "courier new":
            .mono
        case "new york", "ui-serif", "serif", "times", "times new roman", "georgia":
            .serif
        default:
            nil
        }
    }

    /// CSS weight → AppKit weight.
    var appKitWeight: NSFont.Weight {
        let table: [(Double, NSFont.Weight)] = [
            (100, .ultraLight), (200, .thin), (300, .light), (400, .regular), (500, .medium), (600, .semibold),
            (700, .bold), (800, .heavy), (900, .black),
        ]
        let clamped = min(max(weight, 100), 900)
        for index in 0..<(table.count - 1) where clamped <= table[index + 1].0 {
            let (w0, v0) = table[index], (w1, v1) = table[index + 1]
            let t = (clamped - w0) / (w1 - w0)
            return NSFont.Weight(rawValue: v0.rawValue + (v1.rawValue - v0.rawValue) * t)
        }
        return .black
    }
}

/// Measured text: the metrics `ctx.measureText` returns, in units.
struct TextMetrics: Equatable, Sendable {
    var width: Double
    var actualLeft: Double
    var actualRight: Double
    var actualAscent: Double
    var actualDescent: Double
    var fontAscent: Double
    var fontDescent: Double
}

/// Fonts and text layout shared by every drawing VM, the renderer and the validator. Thread-safe:
/// VMs measure text on their own threads.
final class FontCache: @unchecked Sendable {
    static let shared = FontCache()

    private let lock = NSLock()
    private var specs: [String: FontSpec?] = [:]
    private var fonts: [FontSpec: CTFont] = [:]
    /// Base fonts per family/weight/style at 12 pt: creating an AppKit system font costs milliseconds the
    /// first time, deriving another size from a base costs microseconds.
    private var bases: [FontSpec: CTFont] = [:]
    private static let baseSize = 12.0

    /// Creates the regular base font of every family ahead of time, so the first `measureText` or text frame
    /// of a drawing does not blow its 4 ms budget on font loading.
    func warmUp() {
        for family in [FontFamily.system, .rounded, .mono, .serif] {
            for weight in [400.0, 600.0, 700.0] {
                _ = font(for: FontSpec(family: family, weight: weight, italic: false, size: 10, unknownFamily: nil))
            }
        }
    }

    /// The parsed spec for a CSS font string (`nil` when it has no size).
    func spec(for css: String) -> FontSpec? {
        lock.lock()
        defer { lock.unlock() }
        if let cached = specs[css] { return cached }
        let parsed = FontSpec.parse(css)
        if specs.count > 1024 { specs.removeAll(keepingCapacity: true) }
        specs[css] = parsed
        return parsed
    }

    func font(for spec: FontSpec) -> CTFont {
        var key = spec
        key.unknownFamily = nil
        lock.lock()
        if let cached = fonts[key] {
            lock.unlock()
            return cached
        }
        lock.unlock()
        var baseKey = key
        baseKey.size = Self.baseSize
        lock.lock()
        let cachedBase = bases[baseKey]
        lock.unlock()
        let base = cachedBase ?? Self.makeFont(baseKey)
        let font = CTFontCreateCopyWithAttributes(base, CGFloat(key.size), nil, nil)
        lock.lock()
        if cachedBase == nil { bases[baseKey] = base }
        if fonts.count > 256 { fonts.removeAll(keepingCapacity: true) }
        fonts[key] = font
        lock.unlock()
        return font
    }

    private static func makeFont(_ spec: FontSpec) -> CTFont {
        let size = CGFloat(spec.size)
        let weight = spec.appKitWeight
        var font: NSFont
        switch spec.family {
        case .mono:
            font = NSFont.monospacedSystemFont(ofSize: size, weight: weight)
        case .system, .rounded, .serif:
            font = NSFont.systemFont(ofSize: size, weight: weight)
            let design: NSFontDescriptor.SystemDesign? =
                spec.family == .rounded ? .rounded : spec.family == .serif ? .serif : nil
            if let design, let descriptor = font.fontDescriptor.withDesign(design),
               let designed = NSFont(descriptor: descriptor, size: size) {
                font = designed
            }
        }
        if spec.italic {
            let descriptor = font.fontDescriptor.withSymbolicTraits(.italic)
            if let italic = NSFont(descriptor: descriptor, size: size) { font = italic }
        }
        return font as CTFont
    }

    /// A Core Text line whose glyphs take their colour from the context (fill or stroke).
    func line(_ text: String, font: CTFont) -> CTLine {
        let attributes: [CFString: Any] = [
            kCTFontAttributeName: font,
            kCTForegroundColorFromContextAttributeName: kCFBooleanTrue as Any,
        ]
        let string = CFAttributedStringCreate(nil, text as CFString, attributes as CFDictionary)!
        return CTLineCreateWithAttributedString(string)
    }

    func measure(_ text: String, css: String) -> TextMetrics {
        let spec = self.spec(for: css) ?? .default
        let font = self.font(for: spec)
        let line = self.line(text, font: font)
        let width = CTLineGetTypographicBounds(line, nil, nil, nil)
        let bounds = CTLineGetBoundsWithOptions(line, .useGlyphPathBounds)
        let empty = bounds.isNull || bounds.isEmpty
        return TextMetrics(
            width: width,
            actualLeft: empty ? 0 : -Double(bounds.minX),
            actualRight: empty ? 0 : Double(bounds.maxX),
            actualAscent: empty ? 0 : Double(bounds.maxY),
            actualDescent: empty ? 0 : -Double(bounds.minY),
            fontAscent: Double(CTFontGetAscent(font)),
            fontDescent: Double(CTFontGetDescent(font)))
    }
}
