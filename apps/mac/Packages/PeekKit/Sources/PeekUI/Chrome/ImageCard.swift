import CoreGraphics
import PeekCore
import SwiftUI

/// An image with a rounded border (peek in-pactice.jpg: the cover art's dark rim).
struct ImageTileView: View {
    let image: CGImage?
    let size: CGSize
    let shade: PillShade
    var selected = false
    var highlighted = false

    private var radius: CGFloat { min(10, min(size.width, size.height) * 0.12) }

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: radius, style: .continuous)
        Group {
            if let image {
                Image(decorative: image, scale: 1)
                    .resizable()
                    .interpolation(.high)
                    .aspectRatio(contentMode: .fill)
            } else {
                // Unreadable image: keep the space so the layout does not jump.
                Image(systemName: "photo")
                    .font(.system(size: min(size.width, size.height) * 0.3, weight: .regular))
                    .foregroundStyle(shade.secondaryInk)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .background(shade.tint)
            }
        }
        .frame(width: size.width, height: size.height)
        .clipShape(shape)
        .overlay(shape.strokeBorder(shade.border, lineWidth: max(2, min(3, size.width * 0.02))))
        .overlay(shape.strokeBorder(Color.accentColor, lineWidth: selected ? 3 : 0))
        .overlay(shape.inset(by: -3).stroke(shade.ink.opacity(highlighted ? 0.8 : 0), lineWidth: 1.5))
        .shadow(color: .black.opacity(0.22), radius: 6, y: 2)
    }
}

/// A show image and its caption pill underneath, as one element on the arc.
struct ImageCardView: View {
    let image: CGImage?
    let imageSize: CGSize
    let caption: String?
    let size: CGSize
    let shade: PillShade

    /// The caption's glass capsule, or nil without a caption.
    static func plate(caption: String?, size: CGSize, shade: PillShade) -> GlassPlate? {
        guard let caption, !caption.isEmpty else { return nil }
        return .caption(itemSize: size, height: ChromeMetrics.captionHeight, tint: shade.tint)
    }

    var body: some View {
        VStack(spacing: ChromeMetrics.captionGap) {
            ImageTileView(image: image, size: imageSize, shade: shade)
            if let caption, !caption.isEmpty {
                // The capsule behind the caption is the card's glass plate (see `plate`).
                CaptionPillView(text: caption, font: .caption, width: size.width, height: ChromeMetrics.captionHeight,
                                shade: shade)
            }
        }
        .frame(width: size.width, height: size.height, alignment: .top)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(caption ?? "Image")
    }
}
