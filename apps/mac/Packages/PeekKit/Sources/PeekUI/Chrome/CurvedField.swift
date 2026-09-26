import AppKit
import CoreText
import PeekCore
import SwiftUI

/// One line of text set with Core Text, for drawing glyph by glyph along an arc from its start (not centred like
/// ``ArcTextSetter``), with caret offsets for every UTF-16 index.
struct ArcTextRun {
    struct Glyph {
        var glyph: CGGlyph
        /// Left edge from the start of the line.
        var x: CGFloat
        var advance: CGFloat
    }

    let glyphs: [Glyph]
    let width: CGFloat
    private let line: CTLine

    init(_ text: String, font: ChromeFont) {
        let attributed = NSAttributedString(string: text, attributes: [.font: font.nsFont])
        line = CTLineCreateWithAttributedString(attributed)
        width = CGFloat(CTLineGetTypographicBounds(line, nil, nil, nil))
        var glyphs: [Glyph] = []
        for run in (CTLineGetGlyphRuns(line) as? [CTRun]) ?? [] {
            let count = CTRunGetGlyphCount(run)
            guard count > 0 else { continue }
            var ids = [CGGlyph](repeating: 0, count: count)
            var positions = [CGPoint](repeating: .zero, count: count)
            var advances = [CGSize](repeating: .zero, count: count)
            CTRunGetGlyphs(run, CFRange(location: 0, length: count), &ids)
            CTRunGetPositions(run, CFRange(location: 0, length: count), &positions)
            CTRunGetAdvances(run, CFRange(location: 0, length: count), &advances)
            for index in 0..<count {
                glyphs.append(Glyph(glyph: ids[index], x: positions[index].x, advance: advances[index].width))
            }
        }
        self.glyphs = glyphs
    }

    /// The caret's distance from the start of the line before UTF-16 index `index`.
    func offset(utf16 index: Int) -> CGFloat { CTLineGetOffsetForStringIndex(line, index, nil) }
}

/// The band behind the curved field; `extra` thickens it on hover (and thins it on a press), animatably.
struct FieldBandShape: Shape {
    var field: ChromeLayout.CurvedField
    var extra: CGFloat
    var origin: CGPoint

    var animatableData: CGFloat {
        get { extra }
        set { extra = newValue }
    }

    func path(in rect: CGRect) -> Path {
        let half = field.thickness / 2 + extra / 2
        let band = field.arc.band(from: -field.halfLength, to: field.halfLength, inner: -half, outer: half)
        return ArcBandShape(band: band, origin: origin).path(in: rect)
    }
}

/// A text ask's input, curved along the arc right next to its question (ui-feedback.md #7).
///
/// A real text field cannot bend, so editing happens in an invisible `TextField` (focus, keys, IME, undo, Return),
/// and this view draws its text, placeholder, selection and blinking caret glyph by glyph along the band's arc. A
/// click on the band starts typing (or refocuses it); the send button sits at the band's reading end.
struct CurvedFieldView: View {
    @Bindable var model: SlotChromeModel
    let field: ChromeLayout.CurvedField
    let glass: Bool
    @FocusState private var focused: Bool
    /// The caret goes after the seed character (a printable key after the hotkey), never selecting it.
    @State private var selection: TextSelection?
    @State private var pressed = false

    private var typing: Bool { model.content.input == .typing }

    static let font = ChromeFont.field
    static let hintFont = ChromeFont.caption
    static let hint = "or press \\ to talk"

    var body: some View {
        let shade = model.shade
        let feel = ChromeFeel(interactive: true, hovered: model.isHovered(.field),
                              pressed: pressed || model.isPressed(.field), ink: shade.ink)
        let box = field.band.boundingBox.insetBy(dx: -12, dy: -12)
        let shape = FieldBandShape(field: field, extra: (feel.scale - 1) * 30, origin: box.origin)
        ZStack(alignment: .topLeading) {
            if typing {
                TextField("", text: Binding(get: { model.typingText }, set: { model.actions.typingChanged($0) }),
                          selection: $selection)
                    .textFieldStyle(.plain)
                    .font(Self.font.swiftUI)
                    .focused($focused)
                    .onSubmit { model.actions.submitTyping() }
                    .frame(width: 80, height: 22)
                    .opacity(0.01)
                    .position(field.arc.apex)
                    .allowsHitTesting(false)
                    .accessibilityLabel(model.fieldPlaceholder)
                    .onAppear(perform: focus)
            }
            Button(action: tapped) {
                ZStack {
                    shape.fill(Color.black.opacity(feel.shadowOpacity))
                        .frame(width: box.width, height: box.height)
                        .blur(radius: feel.shadowRadius)
                        .offset(y: feel.shadowY)
                    Color.clear
                        .frame(width: box.width, height: box.height)
                        .chromeGlass(shape, tint: shade.tint, glass: glass)
                    shape.stroke(shade.ink.opacity(typing ? max(feel.rimOpacity, 0.45) : feel.rimOpacity),
                                 lineWidth: typing ? max(feel.rimWidth, 1.3) : feel.rimWidth)
                        .frame(width: box.width, height: box.height)
                    TimelineView(.animation(minimumInterval: 1.0 / 30, paused: !(typing && focused))) { timeline in
                        Canvas { context, _ in
                            context.withCGContext { cg in
                                cg.translateBy(x: -box.minX, y: -box.minY)
                                drawContents(in: cg, time: timeline.date.timeIntervalSinceReferenceDate, shade: shade)
                            }
                        }
                    }
                    .frame(width: box.width, height: box.height)
                    .allowsHitTesting(false)
                }
                .frame(width: box.width, height: box.height)
                .contentShape(ArcBandShape(band: field.band, origin: box.origin))
            }
            .buttonStyle(PressReportingStyle(pressed: $pressed))
            .offset(y: -feel.lift)
            .animation(feel.animation, value: feel)
            .position(x: box.midX, y: box.midY)
            .accessibilityLabel(typing && !model.typingText.isEmpty ? model.typingText : model.fieldPlaceholder)
            .accessibilityHint(typing ? "Return sends" : "Starts typing an answer")
            Image(systemName: "keyboard")
                .font(.system(size: 11.5, weight: .semibold))
                .foregroundStyle(shade.secondaryInk)
                .rotationEffect(.radians(field.rotation(at: field.iconS)))
                .position(field.iconCenter)
                .offset(y: -feel.lift)
                .animation(feel.animation, value: feel)
                .allowsHitTesting(false)
            if typing {
                let canSend = !model.typingText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                GlassCircleButton(
                    symbol: "arrow.up", radius: field.submitRadius,
                    tint: canSend ? Color.accentColor.opacity(0.9) : shade.tint, ink: canSend ? .white : shade.secondaryInk,
                    glass: glass, hovered: canSend && model.isHovered(.submit), forcePressed: model.isPressed(.submit),
                    accessibilityLabel: model.context == .testing ? "Send to test silicon" : "Send",
                    help: model.context == .testing ? "Send to test silicon (Return)" : "Send (Return)",
                    action: model.actions.submitTyping
                )
                .disabled(!canSend)
                .position(field.submitCenter)
                .transition(.scale(scale: 0.4).combined(with: .opacity))
            }
        }
        .animation(.spring(duration: 0.35, bounce: 0.2), value: typing)
        .onChange(of: model.focusRequest) { focus() }
    }

    private func tapped() {
        if typing { focus() } else { model.actions.startTyping() }
    }

    private func focus() {
        focused = true
        selection = TextSelection(insertionPoint: model.typingText.endIndex)
    }

    // MARK: Drawing

    private func drawContents(in cg: CGContext, time: Double, shade: PillShade) {
        let available = field.textEnd - field.textStart
        let text = model.typingText
        let caretOn = typing && focused && (time.truncatingRemainder(dividingBy: 1.06) < 0.62)
        if typing, !text.isEmpty {
            let run = ArcTextRun(text, font: Self.font)
            let caretX = run.offset(utf16: caretIndex(in: text))
            // Keep the caret in view: the text scrolls left once it runs past the end of the band.
            let scroll = max(0, caretX - available + 2)
            if let range = selectedRange(in: text), range.lowerBound != range.upperBound {
                let x0 = max(0, run.offset(utf16: range.lowerBound) - scroll)
                let x1 = min(available, run.offset(utf16: range.upperBound) - scroll)
                if x1 > x0 {
                    let band = field.arc.band(from: field.textStart + x0, to: field.textStart + x1,
                                              inner: -Self.font.lineHeight / 2, outer: Self.font.lineHeight / 2)
                    cg.addPath(ArcBandShape(band: band).path(in: .zero).cgPath)
                    cg.setFillColor(NSColor.controlAccentColor.withAlphaComponent(0.38).cgColor)
                    cg.fillPath()
                }
            }
            draw(run, scroll: scroll, available: available, color: shade.inkCGColor, font: Self.font, in: cg)
            if caretOn { drawCaret(at: caretX - scroll, in: cg) }
        } else {
            var room = available
            if !typing {
                let hintWidth = model.measurer.width(of: Self.hint, font: Self.hintFont)
                let placeholderWidth = model.measurer.width(of: model.fieldPlaceholder, font: Self.font)
                if placeholderWidth + 14 + hintWidth <= available {
                    room = available - hintWidth - 14
                    let hint = ArcTextRun(Self.hint, font: Self.hintFont)
                    draw(hint, scroll: -(available - hint.width), available: available,
                         color: shade.inkCGColor.copy(alpha: 0.55) ?? shade.inkCGColor, font: Self.hintFont, in: cg)
                }
            }
            let placeholder = truncate(model.fieldPlaceholder, toWidth: room, font: Self.font, measurer: model.measurer)
            draw(ArcTextRun(placeholder, font: Self.font), scroll: 0, available: room,
                 color: shade.inkCGColor.copy(alpha: 0.62) ?? shade.inkCGColor, font: Self.font, in: cg)
            if caretOn { drawCaret(at: 0, in: cg) }
        }
    }

    /// Glyphs whose box lies within `0…available` after scrolling, set upright along the arc from `textStart`.
    private func draw(_ run: ArcTextRun, scroll: CGFloat, available: CGFloat, color: CGColor, font: ChromeFont,
                      in cg: CGContext) {
        let ctFont = font.nsFont as CTFont
        let capHeight = CTFontGetCapHeight(ctFont)
        cg.setFillColor(color)
        for glyph in run.glyphs {
            let x = glyph.x - scroll
            guard x >= -0.5, x + glyph.advance <= available + 0.5 else { continue }
            let s = field.textStart + x + glyph.advance / 2
            let center = field.arc.point(atArcLength: s)
            cg.saveGState()
            cg.translateBy(x: center.x, y: center.y)
            cg.rotate(by: CGFloat(field.arc.elementRotation(atArcLength: s)))
            cg.scaleBy(x: 1, y: -1)  // Core Text draws y-up; the canvas is y-down
            var id = glyph.glyph
            var position = CGPoint(x: -glyph.advance / 2, y: -capHeight / 2)
            CTFontDrawGlyphs(ctFont, &id, &position, 1, cg)
            cg.restoreGState()
        }
    }

    /// A caret across the band (along the arc's normal) at `x` from the start of the text.
    private func drawCaret(at x: CGFloat, in cg: CGContext) {
        let s = field.textStart + x
        let half = Self.font.lineHeight * 0.48
        let inner = field.arc.point(atArcLength: s, offset: -half)
        let outer = field.arc.point(atArcLength: s, offset: half)
        cg.setStrokeColor(NSColor.controlAccentColor.cgColor)
        cg.setLineWidth(1.6)
        cg.setLineCap(.round)
        cg.move(to: inner)
        cg.addLine(to: outer)
        cg.strokePath()
    }

    /// The caret's UTF-16 index (the end of the selection), clamped to `text`.
    private func caretIndex(in text: String) -> Int {
        selectedRange(in: text)?.upperBound ?? text.utf16.count
    }

    private func selectedRange(in text: String) -> Range<Int>? {
        guard let selection, case .selection(let range) = selection.indices else { return nil }
        func offset(_ index: String.Index) -> Int? {
            guard index <= text.endIndex, let position = index.samePosition(in: text.utf16) else { return nil }
            return text.utf16.distance(from: text.utf16.startIndex, to: position)
        }
        guard let lower = offset(range.lowerBound), let upper = offset(range.upperBound), lower <= upper else { return nil }
        return lower..<upper
    }
}
