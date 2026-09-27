import PeekCore
import SwiftUI

/// Everything peek draws around a Silicon's visual (visual.md B1 "Chrome"): the information arc's
/// pills and images, the question arc, the ask controls, the waveform or typing field, the mic,
/// keyboard and down-arrow buttons, the TEST/SIMULATION badge and ring. Positions come from
/// ``ChromeLayout``; this view only draws them. It fills the whole (never resized) panel.
///
/// There is no background of any kind (ui-feedback.md #8): every element carries its own glass and floats directly
/// over whatever is on screen. On top sit the reveal overlays: the hover tooltip for cut-short text (#1) and the
/// tap-to-expand popup (#5).
public struct SlotChromeView: View {
    @Bindable var model: SlotChromeModel

    public init(model: SlotChromeModel) { self.model = model }

    public var body: some View {
        let layout = model.layout
        let size = model.slotLayout.panelSize
        // Each element is its own glass in both modes: there is no strip or panel for pills to sit on.
        let glass = true
        let tooltip = model.tooltipOverlay
        let popup = model.popupOverlay
        ZStack(alignment: .topLeading) {
            Color.clear.frame(width: size.width, height: size.height)
            if model.isPresented {
                if model.context == .testing {
                    TestRingView(visualRect: layout.visualRect)
                }
                if let question = layout.question {
                    QuestionArcView(model: model, question: question, glass: glass)
                        .transition(.opacity)
                }
                if let field = layout.field {
                    CurvedFieldView(model: model, field: field, glass: glass)
                        .transition(.opacity)
                }
                if let track = layout.track, let role = layout.trackRole {
                    switch role {
                    case .waveform:
                        WaveformView(model: model, track: track, transcribing: model.content.input == .transcribing,
                                     glass: glass)
                            .id(model.content.input == .transcribing ? "transcribing" : "listening")
                    case .slider, .range:
                        ScaleTrackView(model: model, track: track, isRange: role == .range, glass: glass, panelSize: size)
                    }
                }
                ForEach(layout.items, id: \.identity) { item in
                    ChromeItemView(model: model, item: item, glass: glass)
                        // "Esc again to dismiss" fades in and out (0.2 s); everything else also scales a little.
                        .transition(item.kind == .hint && model.content.escHint
                            ? .opacity.animation(.easeInOut(duration: 0.2))
                            : .opacity.combined(with: .scale(scale: 0.9)))
                }
                CornerButtonsView(model: model, buttons: layout.buttons, glass: glass)
                if let tooltip {
                    OverlayTooltipView(overlay: tooltip, shade: model.shade, glass: glass)
                        .id(tooltip.target)
                        .transition(.opacity.combined(with: .scale(scale: 0.94, anchor: tooltip.growAnchor)))
                }
                if let popup {
                    ExpandedPopupView(model: model, overlay: popup, glass: glass)
                        .transition(.asymmetric(
                            insertion: .scale(scale: 0.35, anchor: popup.growAnchor).combined(with: .opacity),
                            removal: .scale(scale: 0.6, anchor: popup.growAnchor).combined(with: .opacity)))
                }
            }
        }
        .frame(width: size.width, height: size.height, alignment: .topLeading)
        .environment(\.colorScheme, model.shade.tone == .dark ? .dark : .light)
        .animation(.smooth(duration: 0.25), value: layout)
        .animation(.easeOut(duration: 0.14), value: tooltip?.target)
        .animation(ChromeOverlayMotion.popupSpring, value: model.expanded)
    }
}

private struct ExpandX: ViewModifier {
    let amount: CGFloat
    func body(content: Content) -> some View { content.scaleEffect(x: amount, y: 1, anchor: .center) }
}

extension AnyTransition {
    /// Grows horizontally from the centre: the keyboard button expanding into the typing field.
    static var expandX: AnyTransition {
        .modifier(active: ExpandX(amount: 0.08), identity: ExpandX(amount: 1)).combined(with: .opacity)
    }
}

/// One row item.
struct ChromeItemView: View {
    @Bindable var model: SlotChromeModel
    let item: ChromeLayout.Item
    let glass: Bool
    /// The item's Button is held down (reported by ``PressReportingStyle``; its glass plate is drawn separately).
    @State private var pressed = false

    private var target: ChromeTarget { .item(item.identity) }

    private var feel: ChromeFeel {
        ChromeFeel(interactive: model.isInteractive(target), hovered: model.isHovered(target),
                   pressed: pressed || model.isPressed(target), ink: model.shade.ink)
    }

    var body: some View {
        let frame = item.frame
        let shade = model.shade
        switch item.kind {
        case .badge:
            if let badge = model.content.badge {
                BadgeView(badge: badge, size: frame.size, tooltip: model.environmentTooltip)
                    .placed(frame, plate: BadgeView.plate(for: badge, size: frame.size), glass: glass)
            }
        case .textPill(let element, let lines):
            if case .text(let text)? = contentElement(at: element) {
                expandable {
                    TextPillView(text: text, lines: lines, size: frame.size, shade: shade)
                }
                .placed(frame, plate: TextPillView.plate(size: frame.size, shade: shade), glass: glass, feel: feel)
            }
        case .imageCard(let element, let imageSize):
            if case .image(let key, _, let caption)? = contentElement(at: element) {
                expandable {
                    ImageCardView(image: model.image(for: key), imageSize: imageSize, caption: caption, size: frame.size,
                                  shade: shade)
                }
                .placed(frame, plate: ImageCardView.plate(caption: caption, size: frame.size, shade: shade), glass: glass,
                        feel: feel)
            }
        case .option(let index, let imageSize):
            if let option = model.option(at: index) {
                let selected = model.isSelected(option.id)
                OptionView(
                    option: option, imageSize: imageSize, size: frame.size, multiple: isMultiple,
                    selected: selected, highlighted: model.highlight == option.id,
                    image: model.image(for: option.imageKey), shade: shade, pressed: $pressed,
                    action: { model.actions.option(option.id) }
                )
                .placed(frame, plate: OptionView.plate(imageSize: imageSize, size: frame.size, selected: selected, shade: shade),
                        glass: glass, feel: feel)
            }
        case .field:
            Group {
                if model.content.input == .typing {
                    TypingFieldView(model: model, size: frame.size)
                        .transition(.expandX)
                } else {
                    TypePromptView(placeholder: model.fieldPlaceholder, size: frame.size, shade: shade, pressed: $pressed,
                                   action: model.actions.startTyping)
                        .transition(.opacity)
                }
            }
            .frame(width: frame.size.width, height: frame.size.height)
            .animation(.spring(duration: 0.35, bounce: 0.15), value: model.content.input)
            // While typing, the (AppKit-backed) text field keeps its rim but is never scaled under the caret.
            .placed(frame, plate: .whole(frame.size, .capsule, tint: shade.tint), glass: glass,
                    feel: model.content.input == .typing ? ChromeFeel(interactive: true, ink: shade.ink) : feel)
        case .confirm:
            // A round button: its ✓ stays upright (a rotated check reads as a "J").
            ConfirmButton(size: frame.size, enabled: model.confirmEnabled, shade: shade, pressed: $pressed,
                          action: model.actions.submitValue)
                .placed(RotatedRect(center: frame.center, size: frame.size, rotation: 0),
                        plate: ConfirmButton.plate(size: frame.size, enabled: model.confirmEnabled, shade: shade),
                        glass: glass, feel: feel)
        case .hint:
            if let hint = model.content.hintText {
                CaptionPillView(text: hint, font: .caption, width: frame.size.width, height: frame.size.height, shade: shade)
                    .placed(frame, plate: .whole(frame.size, .capsule, tint: shade.tint), glass: glass)
            }
        }
    }

    /// Cut-short static text becomes a button that opens the tap-to-expand popup (ui-feedback.md #5).
    @ViewBuilder
    private func expandable(@ViewBuilder _ content: () -> some View) -> some View {
        if model.layout.isExpandable(target) {
            Button { model.toggleExpanded(target) } label: { content().contentShape(Rectangle()) }
                .buttonStyle(PressReportingStyle(pressed: $pressed))
                .accessibilityHint("Shows the whole text")
        } else {
            content()
        }
    }

    private func contentElement(at index: Int) -> ChromeContent.Element? {
        model.content.elements.indices.contains(index) ? model.content.elements[index] : nil
    }

    private var isMultiple: Bool {
        if case .choice(_, let multiple)? = model.content.controls { return multiple }
        return false
    }
}
