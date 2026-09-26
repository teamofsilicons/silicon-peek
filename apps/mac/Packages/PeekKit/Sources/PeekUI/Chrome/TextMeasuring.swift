import AppKit
import CoreText
import Foundation

/// The chrome's type ramp. Every size is in points; the family is always SF Pro (the system font).
public struct ChromeFont: Hashable, Sendable {
    public enum Weight: Sendable, Hashable {
        case regular
        case medium
        case semibold
        case bold
    }

    public var size: CGFloat
    public var weight: Weight

    public init(size: CGFloat, weight: Weight) {
        self.size = size
        self.weight = weight
    }

    /// Show text pills and option labels.
    public static let pill = ChromeFont(size: 12.5, weight: .medium)
    /// Image captions and slider value labels.
    public static let caption = ChromeFont(size: 11.5, weight: .medium)
    /// The `TEST · …` / `SIMULATION` badge.
    public static let badge = ChromeFont(size: 10.5, weight: .semibold)
    /// The question arc, largest size (shrinks toward ``questionMinSize``).
    public static let question = ChromeFont(size: 13.5, weight: .semibold)
    public static let questionMinSize: CGFloat = 11
    /// Notices ("Didn't match an option — tap one or type").
    public static let notice = ChromeFont(size: 12, weight: .semibold)
    /// The typing field.
    public static let field = ChromeFont(size: 13, weight: .regular)

    /// Line height used for multi-line pills and questions.
    public var lineHeight: CGFloat { (size * 1.24).rounded(.up) }

    var nsWeight: NSFont.Weight {
        switch weight {
        case .regular: .regular
        case .medium: .medium
        case .semibold: .semibold
        case .bold: .bold
        }
    }

    /// The AppKit font (SF Pro).
    public var nsFont: NSFont { NSFont.systemFont(ofSize: size, weight: nsWeight) }
}

/// Measures text for the pure chrome layout. The live implementation uses AppKit's text
/// system; tests inject a deterministic one.
public protocol TextMeasuring: Sendable {
    /// Size of `text` set in `font`. With `maxWidth` the text wraps at word boundaries and
    /// is cut to `maxLines` lines; without it the result is one line.
    func size(of text: String, font: ChromeFont, maxWidth: CGFloat?, maxLines: Int) -> CGSize
}

extension TextMeasuring {
    /// Single-line width.
    public func width(of text: String, font: ChromeFont) -> CGFloat {
        size(of: text, font: font, maxWidth: nil, maxLines: 1).width
    }
}

/// Measures with Core Text, which is what SwiftUI's `Text` and the question arc draw with.
public struct SystemTextMeasurer: TextMeasuring {
    public init() {}

    public func size(of text: String, font: ChromeFont, maxWidth: CGFloat?, maxLines: Int) -> CGSize {
        guard !text.isEmpty else { return CGSize(width: 0, height: font.lineHeight) }
        let attributed = NSAttributedString(string: text, attributes: [.font: font.nsFont])
        guard let maxWidth else {
            let line = CTLineCreateWithAttributedString(attributed)
            let width = CGFloat(CTLineGetTypographicBounds(line, nil, nil, nil))
            return CGSize(width: width.rounded(.up) + 1, height: font.lineHeight)
        }
        let framesetter = CTFramesetterCreateWithAttributedString(attributed)
        let path = CGPath(rect: CGRect(x: 0, y: 0, width: maxWidth, height: 100_000), transform: nil)
        let frame = CTFramesetterCreateFrame(framesetter, CFRange(location: 0, length: 0), path, nil)
        let lines = (CTFrameGetLines(frame) as? [CTLine]) ?? []
        let kept = lines.prefix(max(1, maxLines))
        var width: CGFloat = 0
        for line in kept {
            let lineWidth = CGFloat(CTLineGetTypographicBounds(line, nil, nil, nil))
                - CGFloat(CTLineGetTrailingWhitespaceWidth(line))
            width = max(width, lineWidth)
        }
        if lines.count > kept.count { width = maxWidth }  // the last kept line ends in "…"
        return CGSize(width: min(maxWidth, width.rounded(.up) + 1), height: CGFloat(max(1, kept.count)) * font.lineHeight)
    }
}
