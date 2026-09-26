import PeekCore
import SwiftUI

/// Motion for the reveal overlays.
enum ChromeOverlayMotion {
    /// The popup's pop-in and close: a visible bounce that settles quickly (ui-feedback.md #5).
    static let popupSpring = Animation.spring(response: 0.38, dampingFraction: 0.58)
}

extension ChromeOverlay {
    /// The element's centre as a unit point of the overlay's frame: overlays grow out of what they reveal.
    var growAnchor: UnitPoint {
        guard frame.width > 0, frame.height > 0 else { return .center }
        return UnitPoint(x: min(max((origin.x - frame.minX) / frame.width, 0), 1),
                         y: min(max((origin.y - frame.minY) / frame.height, 0), 1))
    }
}

/// The complete text of cut-short chrome while it is hovered (ui-feedback.md #1): a small glass card beside it,
/// shown at once (no tooltip delay), never taking the pointer.
struct OverlayTooltipView: View {
    let overlay: ChromeOverlay
    let shade: PillShade
    let glass: Bool

    var body: some View {
        let frame = overlay.frame
        let shape = RoundedRectangle(cornerRadius: 10, style: .continuous)
        Text(overlay.text)
            .font(overlay.font.swiftUI)
            .foregroundStyle(shade.ink)
            .multilineTextAlignment(.leading)
            .fixedSize(horizontal: false, vertical: true)
            .padding(.horizontal, ChromeMetrics.overlayPadding)
            .frame(width: frame.width, height: frame.height, alignment: .center)
            .chromeGlass(shape, tint: shade.strongFill.opacity(0.8), glass: glass)
            .shadow(color: .black.opacity(0.25), radius: 8, y: 3)
            .position(x: frame.midX, y: frame.midY)
            .allowsHitTesting(false)
            .accessibilityHidden(true)
    }
}

/// Tap-to-expand (ui-feedback.md #5): the whole text in a small glass popup over the element, springing out of it
/// with a bounce. A click on it (or on the element again), a click outside, or Esc closes it with a spring.
struct ExpandedPopupView: View {
    let model: SlotChromeModel
    let overlay: ChromeOverlay
    let glass: Bool
    @State private var pressed = false

    var body: some View {
        let shade = model.shade
        let frame = overlay.frame
        let shape = RoundedRectangle(cornerRadius: 14, style: .continuous)
        let feel = ChromeFeel(interactive: true, hovered: model.isHovered(.popup), pressed: pressed, ink: shade.ink)
        Button {
            model.collapse()
        } label: {
            Group {
                if overlay.scrolls {
                    ScrollView(.vertical) { text }
                        .scrollIndicators(.visible)
                } else {
                    text
                }
            }
            .frame(width: frame.width, height: frame.height)
            .contentShape(shape)
        }
        .buttonStyle(PressReportingStyle(pressed: $pressed))
        .chromeGlass(shape, tint: shade.strongFill.opacity(0.86), glass: glass)
        .overlay(shape.stroke(shade.ink.opacity(feel.hovered ? 0.45 : 0.25), lineWidth: 1))
        .shadow(color: .black.opacity(0.32), radius: 14, y: 5)
        .scaleEffect(pressed ? 0.97 : 1)
        .animation(ChromeFeel.pressSpring, value: pressed)
        .position(x: frame.midX, y: frame.midY)
        .accessibilityLabel(overlay.text)
        .accessibilityHint("Closes the text")
    }

    private var text: some View {
        Text(overlay.text)
            .font(overlay.font.swiftUI)
            .foregroundStyle(model.shade.ink)
            .multilineTextAlignment(.leading)
            .fixedSize(horizontal: false, vertical: true)
            .padding(ChromeMetrics.overlayPadding)
            .frame(width: overlay.frame.width, alignment: .leading)
            .textSelection(.disabled)
    }
}
