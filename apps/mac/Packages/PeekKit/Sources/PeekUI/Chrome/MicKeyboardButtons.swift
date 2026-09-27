import PeekCore
import SwiftUI

/// A round glass button with an SF Symbol, with the interactive rim and springy hover/press (ui-feedback.md #2, #3).
struct GlassCircleButton: View {
    let symbol: String
    let radius: CGFloat
    let tint: Color
    let ink: Color
    let glass: Bool
    var rotation: Double = 0
    var hovered = false
    var forcePressed = false
    let accessibilityLabel: String
    let help: String
    let action: () -> Void
    @State private var pressed = false

    var body: some View {
        Button(action: action) {
            GlassCircleGlyph(symbol: symbol, radius: radius, tint: tint, ink: ink, glass: glass, rotation: rotation,
                             feel: ChromeFeel(interactive: true, hovered: hovered, pressed: pressed || forcePressed, ink: ink))
        }
        .buttonStyle(PressReportingStyle(pressed: $pressed))
        .accessibilityLabel(accessibilityLabel)
        .help(help)
    }
}

/// The circle itself. Its glass is resized for the hover/press spring (never scale-effected), and the glyph rotates
/// inside it, so no glass ever sits under a transform.
struct GlassCircleGlyph: View {
    let symbol: String
    let radius: CGFloat
    let tint: Color
    let ink: Color
    let glass: Bool
    var rotation: Double = 0
    var feel = ChromeFeel.none

    var body: some View {
        let r = radius * feel.scale
        ZStack {
            if feel.interactive {
                Circle()
                    .fill(Color.black.opacity(feel.shadowOpacity))
                    .frame(width: 2 * r, height: 2 * r)
                    .blur(radius: feel.shadowRadius)
                    .offset(y: feel.shadowY)
            }
            Image(systemName: symbol)
                .font(.system(size: radius * 0.86 * feel.scale, weight: .semibold))
                .foregroundStyle(ink)
                .rotationEffect(.radians(rotation))
                .frame(width: 2 * r, height: 2 * r)
                .chromeGlass(Circle(), tint: tint, glass: glass)
                .overlay {
                    if feel.interactive {
                        Circle().strokeBorder(ink.opacity(feel.rimOpacity), lineWidth: feel.rimWidth)
                    }
                }
        }
        .frame(width: 2 * radius * ChromeFeel.hoverScale + 4, height: 2 * radius * ChromeFeel.hoverScale + 4)
        .offset(y: -feel.lift)
        .contentShape(Circle().inset(by: 2))
        .animation(feel.animation, value: feel)
    }
}

/// The mic and keyboard buttons below the visual, and the down-arrow between them
/// (understanding.md "Inputs" and "Interactions"). Pressing mic or keyboard expands the arc into
/// the waveform or the typing field. The down-arrow's clicks are caught by the panel itself
/// (single click = slide out, double click = also stop the audio), so here it only draws; its pressed look
/// comes from the panel through ``SlotChromeModel/pressed``.
struct CornerButtonsView: View {
    let model: SlotChromeModel
    let buttons: ChromeLayout.Buttons
    let glass: Bool

    /// A chevron toward a straight edge; toward a corner a rotated chevron reads as a corner bracket, so it becomes an
    /// arrow there.
    static func downSymbol(rotation: Double) -> String {
        let quarter = (rotation / (.pi / 2)).rounded()
        return abs(rotation - quarter * .pi / 2) < 0.05 ? "chevron.down" : "arrow.down"
    }

    /// The `^` of a compact ask: the down-arrow's glyph turned around (it points away from the edge).
    static func expandSymbol(rotation: Double) -> String {
        downSymbol(rotation: rotation) == "chevron.down" ? "chevron.up" : "arrow.up"
    }

    var body: some View {
        let shade = model.shade
        let listening = model.content.input == .listening
        let typing = model.content.input == .typing
        ZStack(alignment: .topLeading) {
            GlassCircleButton(
                symbol: listening ? "stop.fill" : "mic.fill", radius: buttons.radius,
                tint: listening ? Color.red.opacity(0.85) : shade.tint, ink: listening ? .white : shade.ink, glass: glass,
                hovered: model.isHovered(.mic), forcePressed: model.isPressed(.mic),
                accessibilityLabel: listening ? "Stop and send" : "Answer by voice",
                help: listening ? "Stop and send (\\ or Return)" : "Talk (\\)", action: model.actions.mic
            )
            .position(buttons.mic)
            GlassCircleButton(
                symbol: "keyboard", radius: buttons.radius,
                tint: typing ? Color.accentColor.opacity(0.85) : shade.tint, ink: typing ? .white : shade.ink, glass: glass,
                hovered: model.isHovered(.keyboard), forcePressed: model.isPressed(.keyboard),
                accessibilityLabel: typing ? "Stop typing" : "Type an answer",
                help: typing ? "Stop typing" : "Type (or just start typing)", action: model.actions.keyboard
            )
            .position(buttons.keyboard)
            GlassCircleGlyph(symbol: Self.downSymbol(rotation: buttons.downRotation), radius: buttons.downRadius,
                             tint: shade.tint, ink: shade.ink, glass: glass, rotation: buttons.downRotation,
                             feel: ChromeFeel(interactive: true, hovered: model.isHovered(.down),
                                              pressed: model.isPressed(.down), ink: shade.ink))
                .position(buttons.down)
                .allowsHitTesting(false)
                .help("Close (double-click also stops the voice)")
                .accessibilityLabel("Close")
            if let expand = buttons.expand {
                GlassCircleButton(
                    symbol: Self.expandSymbol(rotation: buttons.expandRotation), radius: buttons.downRadius,
                    tint: shade.tint, ink: shade.ink, glass: glass, rotation: buttons.expandRotation,
                    hovered: model.isHovered(.expand), forcePressed: model.isPressed(.expand),
                    accessibilityLabel: "Expand the question", help: "Expand the question to answer it",
                    action: model.actions.expand
                )
                .position(expand)
                .transition(.scale(scale: 0.4).combined(with: .opacity))
            }
            if let badge = buttons.badge, let size = buttons.badgeSize, model.content.waiting > 0 {
                WaitingBadgeView(count: model.content.waiting, size: size, shade: shade, glass: glass)
                    .position(badge)
                    .allowsHitTesting(false)
                    .transition(.scale(scale: 0.5).combined(with: .opacity))
            }
        }
        .animation(ChromeFeel.spring, value: buttons.expand)
        .animation(ChromeFeel.spring, value: buttons.badge)
    }
}

/// "+N": how many more of this Silicon's peeks wait behind the bubble (peek 0.1.2). Static chrome (no hover reaction,
/// never a target), upright, in the pill shade; it bumps 1.0 → 1.15 → 1.0 when N changes. The glass is resized for the
/// bump, never scale-effected.
struct WaitingBadgeView: View {
    let count: Int
    let size: CGSize
    let shade: PillShade
    let glass: Bool
    @State private var bumped = false

    var body: some View {
        let scale: CGFloat = bumped ? 1.15 : 1
        Text("+\(count)")
            .font(.system(size: ChromeMetrics.waitingBadgeFont.size * scale, weight: .semibold).monospacedDigit())
            .foregroundStyle(shade.ink)
            .lineLimit(1)
            .fixedSize()
            .frame(width: size.width * scale, height: size.height * scale)
            .chromeGlass(Capsule(), tint: shade.strongFill, glass: glass)
            .frame(width: size.width * 1.2, height: size.height * 1.2)
            .animation(ChromeFeel.spring, value: bumped)
            .onChange(of: count) { _, _ in
                bumped = true
                Task { @MainActor in
                    try? await Task.sleep(for: .milliseconds(140))
                    bumped = false
                }
            }
            .accessibilityLabel(count == 1 ? "1 more peek waiting" : "\(count) more peeks waiting")
    }
}
