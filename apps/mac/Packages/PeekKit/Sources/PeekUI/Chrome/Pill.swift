import PeekCore
import SwiftUI

extension ChromeFont {
    /// The SwiftUI font (SF Pro).
    var swiftUI: Font {
        let weight: Font.Weight =
            switch self.weight {
            case .regular: .regular
            case .medium: .medium
            case .semibold: .semibold
            case .bold: .bold
            }
        return .system(size: size, weight: weight)
    }
}

/// A show text element: one line, or a narrow, tall column of lines (ui-feedback.md #4), centred. Its glass pill is
/// the item's plate (``TextPillView/plate``), drawn unrotated so it can follow the arc. When it is cut short, the
/// chrome reveals the rest on hover (glass tooltip) and on click (popup), not with a delayed system tooltip.
struct TextPillView: View {
    let text: String
    let lines: Int
    let size: CGSize
    let shade: PillShade

    static func plate(size: CGSize, shade: PillShade) -> GlassPlate {
        .whole(size, .radius(min(size.height / 2, ChromeMetrics.pillCornerRadius)), tint: shade.tint)
    }

    var body: some View {
        Text(text)
            .font(ChromeFont.pill.swiftUI)
            .foregroundStyle(shade.ink)
            .multilineTextAlignment(.center)
            .lineLimit(max(1, lines))
            .truncationMode(.tail)
            .padding(.horizontal, ChromeMetrics.pillPadding)
            .frame(width: size.width, height: size.height)
            .accessibilityElement(children: .combine)
    }
}

/// One line truncated with "…" (captions, option labels, hints). The capsule behind it is a glass plate drawn by
/// whoever places it (``View/placed(_:plate:glass:feel:)``), because glass must not rotate with the text.
struct CaptionPillView: View {
    let text: String
    let font: ChromeFont
    let width: CGFloat
    let height: CGFloat
    let shade: PillShade
    var ink: Color?

    var body: some View {
        Text(text)
            .font(font.swiftUI)
            .foregroundStyle(ink ?? shade.ink)
            .lineLimit(1)
            .truncationMode(.tail)
            .padding(.horizontal, 8)
            .frame(width: width, height: height)
            .accessibilityLabel(text)
    }
}

/// Identifies the local simulation.
struct BadgeView: View {
    let badge: ChromeContent.Badge
    let size: CGSize
    let tooltip: String?

    static func plate(for badge: ChromeContent.Badge, size: CGSize) -> GlassPlate {
        .whole(size, .capsule, tint: tint(badge))
    }

    static func tint(_ badge: ChromeContent.Badge) -> Color {
        switch badge {
        case .simulation: Color(red: 0.38, green: 0.32, blue: 0.9).opacity(0.78)
        }
    }

    var body: some View {
        Text(badge.text)
            .font(ChromeFont.badge.swiftUI)
            .foregroundStyle(.white)
            .lineLimit(1)
            .truncationMode(.tail)
            .padding(.horizontal, 8)
            .frame(width: size.width, height: size.height)
            .help(tooltip ?? badge.text)
            .accessibilityLabel(badge.text)
    }
}
