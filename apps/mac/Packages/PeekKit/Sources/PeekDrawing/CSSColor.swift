import CoreGraphics
import Foundation

/// A non-premultiplied sRGB colour with components in 0…1.
struct RGBA: Hashable, Sendable {
    var red: Double
    var green: Double
    var blue: Double
    var alpha: Double

    static let transparent = RGBA(red: 0, green: 0, blue: 0, alpha: 0)
    static let black = RGBA(red: 0, green: 0, blue: 0, alpha: 1)
    static let white = RGBA(red: 1, green: 1, blue: 1, alpha: 1)

    var cgColor: CGColor { CGColor(srgbRed: red, green: green, blue: blue, alpha: alpha) }

    /// Rec. 709 luma on the gamma-encoded components: the grey used for vibrant (monochrome) layers.
    var monochrome: RGBA {
        let y = 0.2126 * red + 0.7152 * green + 0.0722 * blue
        return RGBA(red: y, green: y, blue: y, alpha: alpha)
    }

    func withAlpha(_ value: Double) -> RGBA { RGBA(red: red, green: green, blue: blue, alpha: value) }

    /// `#rrggbb`, or `rgba(r, g, b, a)` when translucent.
    var css: String {
        func byte(_ c: Double) -> Int { Int((min(max(c, 0), 1) * 255).rounded()) }
        if alpha >= 1 { return String(format: "#%02x%02x%02x", byte(red), byte(green), byte(blue)) }
        let a = (alpha * 1000).rounded() / 1000
        return "rgba(\(byte(red)), \(byte(green)), \(byte(blue)), \(a))"
    }

    func mix(into hash: inout Hash64) {
        hash.mix(red)
        hash.mix(green)
        hash.mix(blue)
        hash.mix(alpha)
    }
}

/// Parses CSS Color 4 values as canvas accepts them for `fillStyle`, `strokeStyle`, `shadowColor`,
/// gradient stops and glass tints: named colours, `#rgb[a]`, `#rrggbb[aa]`, `rgb[a]()`, `hsl[a]()`
/// (comma or space syntax, `/ alpha`, percentages), `transparent` and `currentcolor` (black).
enum CSSColor {
    static func parse(_ input: String) -> RGBA? {
        let text = input.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard !text.isEmpty else { return nil }
        if text.hasPrefix("#") { return parseHex(text.dropFirst()) }
        if let open = text.firstIndex(of: "("), text.hasSuffix(")") {
            let name = String(text[..<open]).trimmingCharacters(in: .whitespaces)
            let inner = String(text[text.index(after: open)..<text.index(before: text.endIndex)])
            switch name {
            case "rgb", "rgba": return parseRGB(inner)
            case "hsl", "hsla": return parseHSL(inner)
            default: return nil
            }
        }
        if text == "transparent" { return .transparent }
        if text == "currentcolor" { return .black }
        guard let value = named[text] else { return nil }
        return RGBA(red: Double((value >> 16) & 0xff) / 255, green: Double((value >> 8) & 0xff) / 255,
                    blue: Double(value & 0xff) / 255, alpha: 1)
    }

    private static func parseHex(_ digits: Substring) -> RGBA? {
        guard digits.allSatisfy(\.isHexDigit) else { return nil }
        let values = digits.compactMap { $0.hexDigitValue }.map(Double.init)
        switch values.count {
        case 3, 4:
            let a = values.count == 4 ? values[3] * 17 / 255 : 1
            return RGBA(red: values[0] * 17 / 255, green: values[1] * 17 / 255, blue: values[2] * 17 / 255, alpha: a)
        case 6, 8:
            func pair(_ i: Int) -> Double { (values[i] * 16 + values[i + 1]) / 255 }
            return RGBA(red: pair(0), green: pair(2), blue: pair(4), alpha: values.count == 8 ? pair(6) : 1)
        default:
            return nil
        }
    }

    /// Splits `a, b, c[, d]` or `a b c[ / d]` into components.
    private static func components(_ inner: String) -> (values: [String], alpha: String?)? {
        if inner.contains(",") {
            let parts = inner.split(separator: ",", omittingEmptySubsequences: false)
                .map { $0.trimmingCharacters(in: .whitespaces) }
            guard parts.count == 3 || parts.count == 4, parts.allSatisfy({ !$0.isEmpty }) else { return nil }
            return (Array(parts.prefix(3)), parts.count == 4 ? parts[3] : nil)
        }
        let halves = inner.split(separator: "/", omittingEmptySubsequences: false)
        guard halves.count <= 2 else { return nil }
        let values = halves[0].split(whereSeparator: { $0 == " " || $0 == "\t" }).map(String.init)
        guard values.count == 3 else { return nil }
        var alpha: String?
        if halves.count == 2 {
            alpha = halves[1].trimmingCharacters(in: .whitespaces)
            guard alpha?.isEmpty == false else { return nil }
        }
        return (values, alpha)
    }

    private static func number(_ token: String) -> Double? {
        guard let value = Double(token), value.isFinite else { return nil }
        return value
    }

    private static func alphaValue(_ token: String?) -> Double? {
        guard let token else { return 1 }
        if token.hasSuffix("%") {
            guard let v = number(String(token.dropLast())) else { return nil }
            return min(max(v / 100, 0), 1)
        }
        guard let v = number(token) else { return nil }
        return min(max(v, 0), 1)
    }

    private static func parseRGB(_ inner: String) -> RGBA? {
        guard let (values, alpha) = components(inner) else { return nil }
        var channels: [Double] = []
        for token in values {
            if token.hasSuffix("%") {
                guard let v = number(String(token.dropLast())) else { return nil }
                channels.append(min(max(v / 100, 0), 1))
            } else {
                guard let v = number(token) else { return nil }
                channels.append(min(max(v / 255, 0), 1))
            }
        }
        guard let a = alphaValue(alpha) else { return nil }
        return RGBA(red: channels[0], green: channels[1], blue: channels[2], alpha: a)
    }

    private static func parseHSL(_ inner: String) -> RGBA? {
        guard let (values, alpha) = components(inner) else { return nil }
        guard let hue = hueDegrees(values[0]),
              values[1].hasSuffix("%"), let s = number(String(values[1].dropLast())),
              values[2].hasSuffix("%"), let l = number(String(values[2].dropLast())),
              let a = alphaValue(alpha)
        else { return nil }
        let saturation = min(max(s / 100, 0), 1), lightness = min(max(l / 100, 0), 1)
        let h = (hue.truncatingRemainder(dividingBy: 360) + 360).truncatingRemainder(dividingBy: 360) / 360
        func channel(_ n: Double) -> Double {
            let k = (n + h * 12).truncatingRemainder(dividingBy: 12)
            let amount = saturation * min(lightness, 1 - lightness)
            return lightness - amount * max(-1, min(k - 3, 9 - k, 1))
        }
        return RGBA(red: channel(0), green: channel(8), blue: channel(4), alpha: a)
    }

    private static func hueDegrees(_ token: String) -> Double? {
        let units: [(String, Double)] = [("deg", 1), ("grad", 0.9), ("rad", 180 / .pi), ("turn", 360)]
        for (suffix, factor) in units where token.hasSuffix(suffix) {
            return number(String(token.dropLast(suffix.count))).map { $0 * factor }
        }
        return number(token)
    }

    /// CSS named colours (CSS Color 4 §6.1).
    static let named: [String: UInt32] = [
        "aliceblue": 0xf0f8ff, "antiquewhite": 0xfaebd7, "aqua": 0x00ffff, "aquamarine": 0x7fffd4,
        "azure": 0xf0ffff, "beige": 0xf5f5dc, "bisque": 0xffe4c4, "black": 0x000000, "blanchedalmond": 0xffebcd,
        "blue": 0x0000ff, "blueviolet": 0x8a2be2, "brown": 0xa52a2a, "burlywood": 0xdeb887, "cadetblue": 0x5f9ea0,
        "chartreuse": 0x7fff00, "chocolate": 0xd2691e, "coral": 0xff7f50, "cornflowerblue": 0x6495ed,
        "cornsilk": 0xfff8dc, "crimson": 0xdc143c, "cyan": 0x00ffff, "darkblue": 0x00008b, "darkcyan": 0x008b8b,
        "darkgoldenrod": 0xb8860b, "darkgray": 0xa9a9a9, "darkgreen": 0x006400, "darkgrey": 0xa9a9a9,
        "darkkhaki": 0xbdb76b, "darkmagenta": 0x8b008b, "darkolivegreen": 0x556b2f, "darkorange": 0xff8c00,
        "darkorchid": 0x9932cc, "darkred": 0x8b0000, "darksalmon": 0xe9967a, "darkseagreen": 0x8fbc8f,
        "darkslateblue": 0x483d8b, "darkslategray": 0x2f4f4f, "darkslategrey": 0x2f4f4f, "darkturquoise": 0x00ced1,
        "darkviolet": 0x9400d3, "deeppink": 0xff1493, "deepskyblue": 0x00bfff, "dimgray": 0x696969,
        "dimgrey": 0x696969, "dodgerblue": 0x1e90ff, "firebrick": 0xb22222, "floralwhite": 0xfffaf0,
        "forestgreen": 0x228b22, "fuchsia": 0xff00ff, "gainsboro": 0xdcdcdc, "ghostwhite": 0xf8f8ff,
        "gold": 0xffd700, "goldenrod": 0xdaa520, "gray": 0x808080, "green": 0x008000, "greenyellow": 0xadff2f,
        "grey": 0x808080, "honeydew": 0xf0fff0, "hotpink": 0xff69b4, "indianred": 0xcd5c5c, "indigo": 0x4b0082,
        "ivory": 0xfffff0, "khaki": 0xf0e68c, "lavender": 0xe6e6fa, "lavenderblush": 0xfff0f5, "lawngreen": 0x7cfc00,
        "lemonchiffon": 0xfffacd, "lightblue": 0xadd8e6, "lightcoral": 0xf08080, "lightcyan": 0xe0ffff,
        "lightgoldenrodyellow": 0xfafad2, "lightgray": 0xd3d3d3, "lightgreen": 0x90ee90, "lightgrey": 0xd3d3d3,
        "lightpink": 0xffb6c1, "lightsalmon": 0xffa07a, "lightseagreen": 0x20b2aa, "lightskyblue": 0x87cefa,
        "lightslategray": 0x778899, "lightslategrey": 0x778899, "lightsteelblue": 0xb0c4de, "lightyellow": 0xffffe0,
        "lime": 0x00ff00, "limegreen": 0x32cd32, "linen": 0xfaf0e6, "magenta": 0xff00ff, "maroon": 0x800000,
        "mediumaquamarine": 0x66cdaa, "mediumblue": 0x0000cd, "mediumorchid": 0xba55d3, "mediumpurple": 0x9370db,
        "mediumseagreen": 0x3cb371, "mediumslateblue": 0x7b68ee, "mediumspringgreen": 0x00fa9a,
        "mediumturquoise": 0x48d1cc, "mediumvioletred": 0xc71585, "midnightblue": 0x191970, "mintcream": 0xf5fffa,
        "mistyrose": 0xffe4e1, "moccasin": 0xffe4b5, "navajowhite": 0xffdead, "navy": 0x000080, "oldlace": 0xfdf5e6,
        "olive": 0x808000, "olivedrab": 0x6b8e23, "orange": 0xffa500, "orangered": 0xff4500, "orchid": 0xda70d6,
        "palegoldenrod": 0xeee8aa, "palegreen": 0x98fb98, "paleturquoise": 0xafeeee, "palevioletred": 0xdb7093,
        "papayawhip": 0xffefd5, "peachpuff": 0xffdab9, "peru": 0xcd853f, "pink": 0xffc0cb, "plum": 0xdda0dd,
        "powderblue": 0xb0e0e6, "purple": 0x800080, "rebeccapurple": 0x663399, "red": 0xff0000,
        "rosybrown": 0xbc8f8f, "royalblue": 0x4169e1, "saddlebrown": 0x8b4513, "salmon": 0xfa8072,
        "sandybrown": 0xf4a460, "seagreen": 0x2e8b57, "seashell": 0xfff5ee, "sienna": 0xa0522d, "silver": 0xc0c0c0,
        "skyblue": 0x87ceeb, "slateblue": 0x6a5acd, "slategray": 0x708090, "slategrey": 0x708090, "snow": 0xfffafa,
        "springgreen": 0x00ff7f, "steelblue": 0x4682b4, "tan": 0xd2b48c, "teal": 0x008080, "thistle": 0xd8bfd8,
        "tomato": 0xff6347, "turquoise": 0x40e0d0, "violet": 0xee82ee, "wheat": 0xf5deb3, "white": 0xffffff,
        "whitesmoke": 0xf5f5f5, "yellow": 0xffff00, "yellowgreen": 0x9acd32,
    ]
}
