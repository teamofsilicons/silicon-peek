import PeekCore
import SwiftUI

/// A choice: a label pill, or an image card with the label underneath (understanding.md: options can
/// have images). Single choice answers on tap; multiple choice toggles and shows a check.
struct OptionView: View {
    let option: ChromeContent.Option
    let imageSize: CGSize?
    let size: CGSize
    let multiple: Bool
    let selected: Bool
    let highlighted: Bool
    let image: CGImage?
    let shade: PillShade
    @Binding var pressed: Bool
    let action: () -> Void

    /// The option's glass: the label capsule, or the caption capsule under an image.
    static func plate(imageSize: CGSize?, size: CGSize, selected: Bool, shade: PillShade) -> GlassPlate {
        let tint = selected ? Color.accentColor.opacity(0.85) : shade.tint
        if imageSize != nil { return .caption(itemSize: size, height: ChromeMetrics.captionHeight, tint: tint) }
        return .whole(size, .capsule, tint: tint)
    }

    var body: some View {
        Button(action: action) { label }
            .buttonStyle(PressReportingStyle(pressed: $pressed))
            .accessibilityLabel(option.label)
            .accessibilityAddTraits(selected ? [.isSelected, .isButton] : .isButton)
    }

    @ViewBuilder
    private var label: some View {
        if let imageSize {
            VStack(spacing: ChromeMetrics.captionGap) {
                ImageTileView(image: image, size: imageSize, shade: shade, selected: selected, highlighted: highlighted)
                CaptionPillView(text: labelText, font: .caption, width: size.width, height: ChromeMetrics.captionHeight,
                                shade: shade, ink: selected ? .white : nil)
            }
            .frame(width: size.width, height: size.height, alignment: .top)
            .contentShape(Rectangle())
        } else {
            HStack(spacing: 5) {
                if multiple {
                    Image(systemName: selected ? "checkmark.circle.fill" : "circle")
                        .font(.system(size: 12, weight: .semibold))
                        .frame(width: ChromeMetrics.multipleOptionIcon - 5)
                }
                Text(option.label)
                    .font(ChromeFont.pill.swiftUI)
                    .lineLimit(1)
                    .truncationMode(.tail)
            }
            .foregroundStyle(selected ? Color.white : shade.ink)
            .padding(.horizontal, multiple ? ChromeMetrics.multipleOptionPadding : ChromeMetrics.optionPadding)
            .frame(width: size.width, height: size.height)
            .overlay(Capsule().inset(by: -2.5).stroke(shade.ink.opacity(highlighted ? 0.85 : 0), lineWidth: 1.5))
            .contentShape(Capsule())
        }
    }

    private var labelText: String { multiple && selected ? "✓ " + option.label : option.label }
}

/// The ✓ that submits a multiple-choice, slider or range answer.
struct ConfirmButton: View {
    let size: CGSize
    let enabled: Bool
    let shade: PillShade
    @Binding var pressed: Bool
    let action: () -> Void

    static func plate(size: CGSize, enabled: Bool, shade: PillShade) -> GlassPlate {
        .whole(size, .circle, tint: enabled ? Color.accentColor.opacity(0.9) : shade.tint)
    }

    var body: some View {
        Button(action: action) {
            Image(systemName: "checkmark")
                .font(.system(size: 13, weight: .bold))
                .foregroundStyle(enabled ? Color.white : shade.secondaryInk)
                .frame(width: size.width, height: size.height)
                .contentShape(Circle())
        }
        .buttonStyle(PressReportingStyle(pressed: $pressed))
        .disabled(!enabled)
        .accessibilityLabel("Send answer")
        .help("Send answer (Return)")
    }
}

/// The typing field (keyboard button, a printable key after the hotkey, or a text ask's prompt).
/// Return sends; Esc is handled by the panel (cancel and slide back).
struct TypingFieldView: View {
    @Bindable var model: SlotChromeModel
    let size: CGSize
    @FocusState private var focused: Bool
    /// The caret goes after the seed character (a printable key after the hotkey), never selecting it.
    @State private var selection: TextSelection?

    var body: some View {
        let shade = model.shade
        HStack(spacing: 6) {
            TextField(
                "",
                text: Binding(get: { model.typingText }, set: { model.actions.typingChanged($0) }),
                selection: $selection,
                prompt: Text(model.fieldPlaceholder).foregroundStyle(shade.secondaryInk)
            )
            .textFieldStyle(.plain)
            .font(ChromeFont.field.swiftUI)
            .foregroundStyle(shade.ink)
            .focused($focused)
            .onSubmit { model.actions.submitTyping() }
            .accessibilityLabel(model.fieldPlaceholder)
            Button {
                model.actions.submitTyping()
            } label: {
                Image(systemName: "arrow.up.circle.fill")
                    .font(.system(size: 19, weight: .semibold))
                    .foregroundStyle(canSend ? Color.accentColor : shade.secondaryInk)
            }
            .buttonStyle(.plain)
            .disabled(!canSend)
            .accessibilityLabel(model.context == .testing ? "Send to test silicon" : "Send")
            .help(model.context == .testing ? "Send to test silicon (Return)" : "Send (Return)")
        }
        .padding(.leading, 14)
        .padding(.trailing, 7)
        .frame(width: size.width, height: size.height)
        .onAppear { focus() }
        .onChange(of: model.focusRequest) { focus() }
    }

    private func focus() {
        focused = true
        selection = TextSelection(insertionPoint: model.typingText.endIndex)
    }

    private var canSend: Bool { !model.typingText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
}

/// A text ask before the Carbon starts typing: a prompt that opens the field.
struct TypePromptView: View {
    let placeholder: String
    let size: CGSize
    let shade: PillShade
    @Binding var pressed: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            // The "\ to talk" hint only when the placeholder still fits beside it (narrow upright fields drop it).
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 7) {
                    icon
                    Text(placeholder).font(ChromeFont.field.swiftUI).lineLimit(1).fixedSize()
                    Spacer(minLength: 8)
                    Text("or press \\ to talk").font(ChromeFont.caption.swiftUI).lineLimit(1).fixedSize().opacity(0.8)
                }
                HStack(spacing: 7) {
                    icon
                    Text(placeholder).font(ChromeFont.field.swiftUI).lineLimit(1).truncationMode(.tail)
                    Spacer(minLength: 0)
                }
            }
            .foregroundStyle(shade.secondaryInk)
            .padding(.horizontal, 14)
            .frame(width: size.width, height: size.height)
            .contentShape(Capsule())
        }
        .buttonStyle(PressReportingStyle(pressed: $pressed))
        .accessibilityLabel(placeholder)
    }

    private var icon: some View {
        Image(systemName: "keyboard").font(.system(size: 12, weight: .semibold))
    }
}

/// Slider and range: the track takes the whole arc (or line), thumbs are draggable along it, the
/// value rides above the thumb and the bounds sit at the ends.
///
/// ui-feedback.md #6: while dragging, the thumb follows the pointer continuously (the answer value still snaps to the
/// step, so the value pill and ✓ are always valid); on release the thumb springs onto the nearest step. A slider with
/// a sensible number of steps marks each one with a small dot on the track.
struct ScaleTrackView: View {
    @Bindable var model: SlotChromeModel
    let track: ChromeLayout.Track
    let isRange: Bool
    let glass: Bool
    let panelSize: CGSize
    @State private var drag: Drag?

    private enum Thumb: Equatable { case lower, upper, single }

    private struct Drag: Equatable {
        var thumb: Thumb
        /// The pointer's position along the track, unsnapped.
        var t: Double
    }

    typealias Spec = (min: Double, max: Double, step: Double, unit: String?)

    /// The release: an almost-bounce onto the step.
    static let snapSpring = Animation.spring(response: 0.3, dampingFraction: 0.55)
    /// Steps get dots when there are at most this many and they are at least `minDotSpacing` apart.
    static let maxDottedSteps = 24
    static let minDotSpacing: CGFloat = 9

    var body: some View {
        let shade = model.shade
        let spec = model.scaleSpec ?? (min: 0, max: 1, step: 0.01, unit: nil)
        let (v0, v1) = fractions(spec: spec)
        let (t0, t1) = displayed(v0, v1)
        let box = track.boundingBox(thickness: ChromeMetrics.thumbSize + 8)
        let hovered = model.isHovered(.track) || drag != nil
        ZStack(alignment: .topLeading) {
            Color.clear
                .frame(width: box.width, height: box.height)
                .chromeGlass(TrackShape(track: track, thickness: ChromeMetrics.trackThickness + 2, origin: box.origin),
                             tint: shade.tint, glass: glass)
                .position(x: box.midX, y: box.midY)
            TrackShape(track: track, from: isRange ? t0 : 0, to: t1, thickness: ChromeMetrics.trackThickness - 1)
                .fill(Color.accentColor.opacity(0.9))
                .allowsHitTesting(false)
            if let steps = Self.dottedSteps(spec: spec, trackLength: track.length) {
                StepDots(track: track, steps: steps, from: isRange ? t0 : 0, to: t1, ink: shade.ink)
                    .allowsHitTesting(false)
            }
            // The bounds ride on the value row at the ends (clear of screen edges on corner arcs); the one the
            // value pill is near steps aside.
            let valueAt = isRange ? (t0 + t1) / 2 : t1
            boundLabel(model.format(spec.min), at: 0, shade: shade).opacity(valueAt < 0.18 ? 0 : 1)
            boundLabel(model.format(spec.max), at: 1, shade: shade).opacity(valueAt > 0.82 ? 0 : 1)
            if isRange {
                ThumbView(track: track, t: t0, glass: glass, feel: thumbFeel(.lower, hovered: hovered, ink: shade.ink))
            }
            ThumbView(track: track, t: t1, glass: glass,
                      feel: thumbFeel(isRange ? .upper : .single, hovered: hovered, ink: shade.ink))
            ValuePillView(track: track, t: isRange ? (t0 + t1) / 2 : t1, text: valueText(spec: spec),
                          width: model.measurer.width(of: valueText(spec: spec), font: .caption) + 18, shade: shade,
                          glass: glass)
        }
        .frame(width: panelSize.width, height: panelSize.height, alignment: .topLeading)
        .contentShape(TrackShape(track: track, thickness: ChromeMetrics.thumbSize + 8))
        .gesture(
            DragGesture(minimumDistance: 0)
                .onChanged { gesture in changed(to: gesture.location, spec: spec, v0: v0, v1: v1) }
                .onEnded { _ in
                    withAnimation(Self.snapSpring) { drag = nil }
                    model.actions.dragging(false)
                }
        )
        // Keyboard nudges and typed answers move the thumbs with the same spring; a drag moves them directly.
        .animation(drag == nil ? Self.snapSpring : nil, value: model.askValue)
        .onAppear(perform: applyDebugDrag)
        .onChange(of: model.debugDrag) { applyDebugDrag() }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(isRange ? "Range" : "Slider")
        .accessibilityValue(valueText(spec: spec))
        .accessibilityAdjustableAction { direction in adjust(direction == .increment ? 1 : -1, spec: spec) }
    }

    /// The number of steps to mark with dots, or nil for a (practically) continuous slider.
    static func dottedSteps(spec: Spec, trackLength: CGFloat) -> Int? {
        guard spec.step > 0, spec.max > spec.min else { return nil }
        let raw = (spec.max - spec.min) / spec.step
        let steps = Int(raw.rounded())
        guard abs(raw - Double(steps)) < 1e-6, steps >= 2, steps <= maxDottedSteps,
              trackLength / CGFloat(steps) >= minDotSpacing else { return nil }
        return steps
    }

    /// The thumbs' fractions from the answer value (snapped to the step).
    private func fractions(spec: Spec) -> (Double, Double) {
        let span = max(spec.max - spec.min, .ulpOfOne)
        switch model.askValue {
        case .number(let value)?:
            return (0, min(max((value - spec.min) / span, 0), 1))
        case .range(let lower, let upper)?:
            return (min(max((lower - spec.min) / span, 0), 1), min(max((upper - spec.min) / span, 0), 1))
        default:
            return (0, isRange ? 1 : 0)
        }
    }

    /// What is drawn: the dragged thumb at the pointer, the others at their values.
    private func displayed(_ v0: Double, _ v1: Double) -> (Double, Double) {
        guard let drag else { return (v0, v1) }
        switch drag.thumb {
        case .lower: return (min(drag.t, v1), v1)
        case .upper: return (v0, max(drag.t, v0))
        case .single: return (v0, drag.t)
        }
    }

    private func thumbFeel(_ thumb: Thumb, hovered: Bool, ink: Color) -> ChromeFeel {
        ChromeFeel(interactive: true, hovered: hovered && (drag == nil || drag?.thumb == thumb),
                   pressed: false, ink: ink)
    }

    private func boundLabel(_ text: String, at t: Double, shade: PillShade) -> some View {
        Text(text)
            .font(ChromeFont.caption.swiftUI)
            .foregroundStyle(shade.ink)
            .shadow(color: shade.tone == .dark ? .white.opacity(0.6) : .black.opacity(0.6), radius: 2)
            .fixedSize()
            .rotationEffect(.radians(track.rotation(at: t)))
            .position(track.point(at: t, lift: ChromeMetrics.thumbSize / 2 + ChromeMetrics.valuePillGap
                                    + ChromeMetrics.captionHeight / 2))
            .allowsHitTesting(false)
    }

    private func valueText(spec: Spec) -> String {
        switch model.askValue {
        case .number(let value)?: model.format(value)
        case .range(let lower, let upper)?: "\(model.format(lower)) – \(model.format(upper))"
        default: model.format(spec.min)
        }
    }

    private func changed(to location: CGPoint, spec: Spec, v0: Double, v1: Double) {
        let t = track.fraction(nearest: location)
        var thumb = drag?.thumb
        if thumb == nil {
            model.actions.dragging(true)
            thumb = isRange ? (abs(t - v0) <= abs(t - v1) ? .lower : .upper) : .single
            // Two thumbs on top of each other: pick by the side of the press.
            if isRange, abs(v0 - v1) < 0.001 { thumb = t < v0 ? .lower : .upper }
        }
        guard let thumb else { return }
        drag = Drag(thumb: thumb, t: t)
        setValue(at: t, thumb: thumb, spec: spec)
    }

    /// The answer value for a thumb at `t`, snapped to the step.
    private func setValue(at t: Double, thumb: Thumb, spec: Spec) {
        let raw = spec.min + t * (spec.max - spec.min)
        let value = TypedAnswerMatcher.clampAndSnap(raw, min: spec.min, max: spec.max, step: spec.step)
        switch (thumb, model.askValue) {
        case (.lower, .range(_, let upper)?):
            model.actions.setValue(.range(lower: min(value, upper), upper: upper))
        case (.upper, .range(let lower, _)?):
            model.actions.setValue(.range(lower: lower, upper: max(value, lower)))
        case (.lower, _), (.upper, _):
            model.actions.setValue(.range(lower: min(value, spec.max), upper: spec.max))
        case (.single, _):
            model.actions.setValue(.number(value))
        }
    }

    private func adjust(_ direction: Double, spec: Spec) {
        switch model.askValue {
        case .number(let value)?:
            model.actions.setValue(.number(TypedAnswerMatcher.clampAndSnap(value + direction * spec.step, min: spec.min,
                                                                           max: spec.max, step: spec.step)))
        case .range(let lower, let upper)?:
            let next = TypedAnswerMatcher.clampAndSnap(upper + direction * spec.step, min: lower, max: spec.max, step: spec.step)
            model.actions.setValue(.range(lower: lower, upper: next))
        default:
            break
        }
    }

    /// `PEEK_DEBUG_DRAG` (Debug builds): hold the (upper) thumb mid-drag at that fraction, for screenshots.
    private func applyDebugDrag() {
        guard let t = model.debugDrag, let spec = model.scaleSpec else { return }
        let thumb: Thumb = isRange ? .upper : .single
        drag = Drag(thumb: thumb, t: min(max(t, 0), 1))
        setValue(at: min(max(t, 0), 1), thumb: thumb, spec: spec)
    }
}

/// A slider/range thumb at fraction `t` of the track. Animatable in `t`, so the release spring slides it along the
/// arc (not across the chord) onto its step.
struct ThumbView: View, Animatable {
    let track: ChromeLayout.Track
    var t: Double
    let glass: Bool
    let feel: ChromeFeel

    nonisolated var animatableData: Double {
        get { t }
        set { t = newValue }
    }

    var body: some View {
        // Grows on hover and while held (a grab), springing back; the glass circle is resized, never scale-effected.
        let size = ChromeMetrics.thumbSize * (feel.hovered ? 1.18 : 1)
        Circle()
            .fill(Color.white.opacity(0.001))
            .frame(width: size, height: size)
            .chromeGlass(Circle(), tint: Color.white.opacity(0.62), glass: glass)
            .overlay(Circle().strokeBorder(Color.accentColor.opacity(0.95), lineWidth: feel.hovered ? 2.5 : 2))
            .shadow(color: .black.opacity(feel.hovered ? 0.35 : 0.25), radius: feel.hovered ? 6 : 3, y: feel.hovered ? 2.5 : 1)
            .animation(ChromeFeel.spring, value: feel.hovered)
            .position(track.point(at: t))
            .allowsHitTesting(false)
    }
}

/// The value pill riding above the thumb (animatable in `t` with it).
struct ValuePillView: View, Animatable {
    let track: ChromeLayout.Track
    var t: Double
    let text: String
    let width: CGFloat
    let shade: PillShade
    let glass: Bool

    nonisolated var animatableData: Double {
        get { t }
        set { t = newValue }
    }

    var body: some View {
        let size = CGSize(width: width, height: ChromeMetrics.captionHeight)
        let frame = RotatedRect(
            center: track.point(at: t, lift: ChromeMetrics.thumbSize / 2 + ChromeMetrics.valuePillGap
                                    + ChromeMetrics.captionHeight / 2),
            size: size, rotation: track.rotation(at: t))
        CaptionPillView(text: text, font: .caption, width: width, height: ChromeMetrics.captionHeight, shade: shade)
            .placed(frame, plate: .whole(size, .capsule, tint: shade.tint), glass: glass)
            .allowsHitTesting(false)
    }
}

/// A small dot at every step of a stepped slider/range; dots under the filled part are white.
struct StepDots: View {
    let track: ChromeLayout.Track
    let steps: Int
    let from: Double
    let to: Double
    let ink: Color

    var body: some View {
        ForEach(0...steps, id: \.self) { index in
            let t = Double(index) / Double(steps)
            let filled = t >= from - 1e-9 && t <= to + 1e-9
            Circle()
                .fill(filled ? Color.white.opacity(0.95) : ink.opacity(0.55))
                .frame(width: 3.5, height: 3.5)
                .position(track.point(at: t))
        }
    }
}

/// The live waveform while recording (from `mic.level`), and a travelling shimmer while transcribing.
/// It fills the whole arc: the mic button "expands in X and takes up the entire space".
struct WaveformView: View {
    let model: SlotChromeModel
    let track: ChromeLayout.Track
    let transcribing: Bool
    let glass: Bool
    @State private var grown = false

    var body: some View {
        let shade = model.shade
        let box = track.boundingBox(thickness: ChromeMetrics.waveformThickness + 4)
        let band = TrackShape(track: track, from: grown ? 0 : 0.47, to: grown ? 1 : 0.53,
                              thickness: ChromeMetrics.waveformThickness, origin: box.origin)
        ZStack {
            Color.clear
                .frame(width: box.width, height: box.height)
                .chromeGlass(band, tint: transcribing ? shade.tint : Color.red.opacity(shade.tone == .dark ? 0.35 : 0.25),
                             glass: glass)
            // Read the levels here, in `body`, so each new mic level re-renders the waveform.
            let levels = model.micLevels
            TimelineView(.animation(minimumInterval: 1 / 30, paused: !transcribing)) { timeline in
                Canvas { context, _ in
                    draw(in: &context, levels: levels, origin: box.origin, time: timeline.date.timeIntervalSinceReferenceDate,
                         shade: shade)
                }
            }
            .frame(width: box.width, height: box.height)
            .opacity(grown ? 1 : 0)
            if transcribing {
                Text("Transcribing…")
                    .font(ChromeFont.caption.swiftUI)
                    .foregroundStyle(shade.ink)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 2)
                    .background(shade.tint, in: Capsule())
                    .rotationEffect(.radians(track.rotation(at: 0.5)))
                    .position(x: track.point(at: 0.5).x - box.minX, y: track.point(at: 0.5).y - box.minY)
            }
        }
        .frame(width: box.width, height: box.height)
        .position(x: box.midX, y: box.midY)
        .allowsHitTesting(false)
        .onAppear { withAnimation(.spring(duration: 0.45, bounce: 0.18)) { grown = true } }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(transcribing ? "Transcribing" : "Recording")
    }

    private func draw(in context: inout GraphicsContext, levels: [Double], origin: CGPoint, time: Double, shade: PillShade) {
        let count = max(8, Int(track.length / 6))
        let maxLength = ChromeMetrics.waveformThickness - 12
        var path = Path()
        for index in 0..<count {
            let t = 0.04 + 0.92 * Double(index) / Double(count - 1)
            let level: Double
            if transcribing {
                let phase = (time * 0.8).truncatingRemainder(dividingBy: 1)
                let distance = abs(t - phase)
                level = 0.12 + 0.55 * max(0, 1 - distance * 7)
            } else {
                // Newest samples on the right (reading end).
                let sample = Int(Double(levels.count - 1) * Double(index) / Double(count - 1))
                level = levels[max(0, min(levels.count - 1, sample))]
            }
            let length = 3 + CGFloat(min(1, max(0, level)).squareRoot()) * maxLength
            let center = track.point(at: t)
            let normal = track.normal(at: t)
            let dx = normal.dx * length / 2, dy = normal.dy * length / 2
            path.move(to: CGPoint(x: center.x - origin.x - dx, y: center.y - origin.y - dy))
            path.addLine(to: CGPoint(x: center.x - origin.x + dx, y: center.y - origin.y + dy))
        }
        context.stroke(path, with: .color(shade.ink.opacity(0.9)), style: StrokeStyle(lineWidth: 2.5, lineCap: .round))
    }
}
