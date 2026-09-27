import AppKit
import CoreText
import PeekCore
import SwiftUI

/// Where each glyph of a line goes on its arc (pure; the view draws these).
struct ArcGlyphPlacement: Equatable {
    var glyph: CGGlyph
    /// Glyph centre on the line's middle circle (panel-local).
    var center: CGPoint
    var rotation: Double
    var advance: CGFloat
}

enum ArcTextSetter {
    /// Places the glyphs of `text` in `font` along `arc`, centred on its midpoint, in reading order.
    /// Glyphs are set from Core Text runs, so kerning and ligatures match straight text.
    static func place(_ text: String, font: ChromeFont, on arc: InfoArc) -> [ArcGlyphPlacement] {
        let attributed = NSAttributedString(string: text, attributes: [.font: font.nsFont])
        let line = CTLineCreateWithAttributedString(attributed)
        let width = CGFloat(CTLineGetTypographicBounds(line, nil, nil, nil))
        guard let runs = CTLineGetGlyphRuns(line) as? [CTRun] else { return [] }
        var placements: [ArcGlyphPlacement] = []
        for run in runs {
            let count = CTRunGetGlyphCount(run)
            guard count > 0 else { continue }
            var glyphs = [CGGlyph](repeating: 0, count: count)
            var positions = [CGPoint](repeating: .zero, count: count)
            var advances = [CGSize](repeating: .zero, count: count)
            CTRunGetGlyphs(run, CFRange(location: 0, length: count), &glyphs)
            CTRunGetPositions(run, CFRange(location: 0, length: count), &positions)
            CTRunGetAdvances(run, CFRange(location: 0, length: count), &advances)
            for index in 0..<count {
                let s = -width / 2 + positions[index].x + advances[index].width / 2
                placements.append(ArcGlyphPlacement(
                    glyph: glyphs[index], center: arc.point(atArcLength: s),
                    rotation: arc.elementRotation(atArcLength: s), advance: advances[index].width))
            }
        }
        return placements
    }
}

/// The question (or a notice) on its own arc above the answer controls: a curved glass band with
/// the text set glyph by glyph along it (understanding.md: "question is its own arc above, and is curved").
/// A question cut short with "…" is expandable text (ui-feedback.md #1, #5): it reveals itself on hover and opens
/// the popup on click, and looks interactive (rim, springy hover) meanwhile.
struct QuestionArcView: View {
    let model: SlotChromeModel
    let question: ChromeLayout.Question
    let glass: Bool
    @State private var pressed = false

    private var shade: PillShade { model.shade }

    private var feel: ChromeFeel {
        ChromeFeel(interactive: model.isInteractive(.question), hovered: model.isHovered(.question),
                   pressed: pressed || model.isPressed(.question), ink: shade.ink)
    }

    var body: some View {
        if model.content.askCollapsed {
            // The compact ask: a click on its question brings the answer controls back.
            Button { model.questionClicked() } label: { band }
                .buttonStyle(PressReportingStyle(pressed: $pressed))
                .accessibilityHint("Expands the question to answer it")
        } else if model.layout.isExpandable(.question) {
            Button { model.questionClicked() } label: { band }
                .buttonStyle(PressReportingStyle(pressed: $pressed))
                .accessibilityHint("Shows the whole question")
        } else {
            band
        }
    }

    @ViewBuilder
    private var band: some View {
        switch question.background {
        case .arc(let band):
            let feel = self.feel
            let grown = feel.interactive ? ArcBand(center: band.center, innerRadius: band.innerRadius - (feel.scale - 1) * 20,
                                                   outerRadius: band.outerRadius + (feel.scale - 1) * 20,
                                                   startAngle: band.startAngle, endAngle: band.endAngle) : band
            let box = band.boundingBox.insetBy(dx: -8, dy: -8)
            let shape = ArcBandShape(band: grown, origin: box.origin)
            ZStack {
                if feel.interactive {
                    shape.fill(Color.black.opacity(feel.shadowOpacity))
                        .frame(width: box.width, height: box.height)
                        .blur(radius: feel.shadowRadius)
                        .offset(y: feel.shadowY)
                }
                Color.clear
                    .frame(width: box.width, height: box.height)
                    .chromeGlass(shape, tint: bandTint, glass: glass)
                if feel.interactive {
                    shape.stroke(feel.ink.opacity(feel.rimOpacity), lineWidth: feel.rimWidth)
                        .frame(width: box.width, height: box.height)
                }
                Canvas { context, _ in
                    context.withCGContext { cg in
                        cg.translateBy(x: -box.minX, y: -box.minY)
                        for line in question.lines { draw(line, in: cg) }
                    }
                }
                .frame(width: box.width, height: box.height)
                .allowsHitTesting(false)
            }
            .frame(width: box.width, height: box.height)
            .contentShape(ArcBandShape(band: band, origin: box.origin))
            .offset(y: -feel.lift)
            .animation(feel.animation, value: feel)
            .position(x: box.midX, y: box.midY)
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(question.fullText.isEmpty ? question.lines.map(\.text).joined(separator: " ") : question.fullText)
        case .rect(let rect):
            let lines = question.lines.first
            Text(question.lines.map(\.text).joined(separator: " "))
                .font((lines?.font ?? .question).swiftUI)
                .foregroundStyle(question.isNotice ? Color.white : shade.ink)
                .multilineTextAlignment(.center)
                .lineLimit(blockLines)
                .truncationMode(.tail)
                .padding(.horizontal, 8)
                .contentShape(Rectangle())
                .placed(rect, plate: .whole(rect.size, .radius(12), tint: bandTint), glass: glass, feel: feel)
        }
    }

    private var bandTint: Color {
        question.isNotice ? Color.orange.opacity(shade.tone == .dark ? 0.7 : 0.55) : shade.tint
    }

    private var blockLines: Int {
        if case .block(_, let lines)? = question.lines.first?.placement { return max(1, lines) }
        return 1
    }

    private func draw(_ line: ChromeLayout.QuestionLine, in cg: CGContext) {
        guard case .arc(let arc, _) = line.placement else { return }
        let ctFont = line.font.nsFont as CTFont
        let capHeight = CTFontGetCapHeight(ctFont)
        cg.setFillColor(question.isNotice ? CGColor(gray: 1, alpha: 1) : shade.inkCGColor)
        for placement in ArcTextSetter.place(line.text, font: line.font, on: arc) {
            cg.saveGState()
            cg.translateBy(x: placement.center.x, y: placement.center.y)
            cg.rotate(by: CGFloat(placement.rotation))
            cg.scaleBy(x: 1, y: -1)  // Core Text draws y-up; the canvas is y-down
            var glyph = placement.glyph
            var position = CGPoint(x: -placement.advance / 2, y: -capHeight / 2)
            CTFontDrawGlyphs(ctFont, &glyph, &position, 1, cg)
            cg.restoreGState()
        }
    }
}
