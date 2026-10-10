import CoreGraphics
import Foundation
import PeekCore

/// One validation frame: the events delivered before it and its input.
struct ValidationStep: Sendable {
    var index: Int
    var events: [DrawingEvent]
    var input: InputSnapshot

    /// `phase=showing, show=null, …` (visual.md A9 failure output).
    var summary: String { ValidationSchedule.summary(of: input) }
}

/// The 90 offscreen test frames of visual.md A9, as amended by BLUEPRINT §0.1 (no words, no transcript):
/// every phase (incl. `transcribing`), normal and compact, light and dark appearance and backdrop, a show
/// with a sample image and text, an ask of each type with a moving value, speech and mic levels as sine
/// waves, hover on and off with the pointer circling, one click, one move, every context and both glass
/// modes, plus enter / send / answer / leave events.
///
/// Only combinations a real bubble can have: a send shows (frames 0–29, spoken over in 20–29) or asks (30–79,
/// `show` null, as `--show` and `--ask` never go together); `speech` exists only while that send speaks.
enum ValidationSchedule {
    static let frameCount = DrawingLimits.validationFrames
    static let frameInterval = 1.0 / 60

    /// Phases in 10-frame blocks.
    static let phases: [Phase] = [
        .entering, .showing, .speaking, .asking, .listening, .typing, .transcribing, .leaving, .hidden,
    ]

    static let coverHandle = ImageHandle(id: 1, width: 96, height: 96)
    static let optionHandles = [ImageHandle(id: 2, width: 64, height: 64), ImageHandle(id: 3, width: 64, height: 64)]
    static let coverColors = ImageColors(dominant: "#c8352b", palette: ["#c8352b", "#f2a541", "#2d1e2f", "#f7e8d0"])
    static let optionColors = [
        ImageColors(dominant: "#2a9d8f", palette: ["#2a9d8f", "#264653", "#e9c46a"]),
        ImageColors(dominant: "#6d597a", palette: ["#6d597a", "#b56576", "#eaac8b"]),
    ]

    /// Decoded sample images for the handles above (cover art and two option thumbnails).
    static func sampleImages() -> [Int: CGImage] {
        var images: [Int: CGImage] = [:]
        images[coverHandle.id] = makeArt(size: coverHandle.width, top: (0.95, 0.65, 0.25), bottom: (0.78, 0.2, 0.17))
        images[optionHandles[0].id] = makeArt(size: optionHandles[0].width, top: (0.16, 0.62, 0.56),
                                              bottom: (0.15, 0.27, 0.33))
        images[optionHandles[1].id] = makeArt(size: optionHandles[1].width, top: (0.71, 0.4, 0.45),
                                              bottom: (0.43, 0.35, 0.48))
        return images
    }

    private static func makeArt(size: Int, top: (Double, Double, Double), bottom: (Double, Double, Double)) -> CGImage {
        let space = CGReplay.sRGB
        let context = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: 0, space: space,
                                bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
        let colors = [
            CGColor(srgbRed: top.0, green: top.1, blue: top.2, alpha: 1),
            CGColor(srgbRed: bottom.0, green: bottom.1, blue: bottom.2, alpha: 1),
        ] as CFArray
        let gradient = CGGradient(colorsSpace: space, colors: colors, locations: [0, 1])!
        let s = CGFloat(size)
        context.drawLinearGradient(gradient, start: CGPoint(x: 0, y: s), end: CGPoint(x: s, y: 0), options: [])
        context.setFillColor(CGColor(srgbRed: 1, green: 1, blue: 1, alpha: 0.85))
        context.fillEllipse(in: CGRect(x: s * 0.3, y: s * 0.3, width: s * 0.4, height: s * 0.4))
        context.setFillColor(CGColor(srgbRed: 0.1, green: 0.1, blue: 0.1, alpha: 1))
        context.fillEllipse(in: CGRect(x: s * 0.46, y: s * 0.46, width: s * 0.08, height: s * 0.08))
        return context.makeImage()!
    }

    // MARK: Steps

    static func steps(glass: GlassMode) -> [ValidationStep] {
        (0..<frameCount).map { step(at: $0, glass: glass) }
    }

    static let show = InputSnapshot.Show(elements: [
        .text("Tonight's mix"),
        .image(coverHandle, caption: "Side A", colors: coverColors),
    ])

    // swiftlint:disable:next function_body_length
    static func step(at i: Int, glass: GlassMode) -> ValidationStep {
        let t = Double(i) * frameInterval
        let dt = i == 0 ? 0 : frameInterval
        let phase = phases[min(i / 10, phases.count - 1)]
        let mode: DisplayMode = (15..<25).contains(i) || (65..<75).contains(i) ? .compact : .normal
        let appearance: Appearance = (i / 15) % 2 == 1 ? .dark : .light
        let backdropDark = (i / 23) % 2 == 1
        let backdrop = backdropDark
            ? Backdrop(tone: .dark, luminance: 0.12, color: "#1f2430", source: .wallpaper)
            : Backdrop(tone: .light, luminance: 0.86, color: "#dfe6ee", source: .wallpaper)
        let moved = i >= 55
        let slot = InputSnapshot.Slot(index: moved ? .right : .bottom, facing: moved ? .pi : -.pi / 2)

        let hover = (25..<45).contains(i) || (60..<65).contains(i)
        let angle = Double(i) * 0.35
        let radius = hover ? 30.0 : 90.0
        let mouse = InputSnapshot.Mouse(x: 50 + cos(angle) * radius, y: 50 + sin(angle) * radius)

        let wave = 0.5 + 0.5 * sin(Double(i) * 0.9)
        var speech: InputSnapshot.Speech?
        if phase == .speaking {
            speech = InputSnapshot.Speech(text: "Here is the mix for tonight, enjoy it", level: wave,
                                          progress: min(1, Double(i - 20) / 9), done: i == 29)
        }
        let micLevel = phase == .listening ? 0.5 + 0.5 * sin(Double(i) * 1.3) : 0
        let typing = phase == .typing ? InputSnapshot.Typing(text: String("on my way".prefix(i - 49))) : nil
        let show = i < 30 ? Self.show : nil
        let ask = self.ask(at: i)

        let context: InputContext = (80..<90).contains(i) ? .simulation : .production
        let glassMode: GlassMode = glass == .live && (30..<40).contains(i) ? .frosted : glass

        let input = InputSnapshot(
            t: t, dt: dt, slot: slot, mode: mode, appearance: appearance, backdrop: backdrop, phase: phase,
            hover: hover, mouse: mouse, speech: speech, mic: .init(level: micLevel), typing: typing, show: show,
            ask: ask, context: context, glass: glassMode)

        var events: [DrawingEvent] = []
        switch i {
        case 0:
            events = [.enter, .send(show: show, ask: nil, speech: nil)]
        case 20:
            events = [.send(show: show, ask: nil, speech: speech)]
        case 30, 40, 50, 60, 70:
            events = [.send(show: show, ask: ask, speech: speech)]
        case 25:
            events = [.click(x: mouse.x, y: mouse.y, count: 1)]
        case 38:
            events = [.answer(value: .text("sounds good"), via: .keyboard)]
        case 48:
            events = [.answer(value: .choice("2"), via: .click)]
        case 55:
            events = [.move(from: .bottom, to: .right)]
        case 68:
            events = [.answer(value: .number(77), via: .voice)]
        case 75:
            events = [.leave]
        default:
            break
        }
        return ValidationStep(index: i, events: events, input: input)
    }

    private static func ask(at i: Int) -> InputSnapshot.Ask? {
        let k = Double(i % 10)
        switch i {
        case 30..<40:
            return .init(question: "Coming tonight?", type: .text, value: .text(String("sounds good".prefix(i - 29))))
        case 40..<50:
            let options = [
                InputSnapshot.Ask.Option(id: "1", label: "Yes", image: optionHandles[0], colors: optionColors[0]),
                InputSnapshot.Ask.Option(id: "2", label: "No", image: optionHandles[1], colors: optionColors[1]),
                InputSnapshot.Ask.Option(id: "3", label: "Later"),
            ]
            return .init(question: "Play side B?", type: .singleChoice, options: options,
                         value: i < 45 ? nil : .choice("2"), highlight: ["1", "2", "3"][i % 3])
        case 50..<60:
            let options = ["Drums", "Bass", "Keys", "Voice"].enumerated().map {
                InputSnapshot.Ask.Option(id: String($0.offset + 1), label: $0.element)
            }
            let chosen = (0..<(1 + (i - 50) / 3)).map { String($0 + 1) }
            return .init(question: "Which stems?", type: .multipleChoice, options: options,
                         value: .choices(chosen), highlight: String(1 + i % 4))
        case 60..<70:
            return .init(question: "Volume?", type: .slider, min: 0, max: 100, step: 1, value: .number(k * 11))
        case 70..<80:
            return .init(question: "Which hours?", type: .range, min: 0, max: 10, step: 0.5,
                         value: .range(lower: k * 0.5, upper: 10 - k * 0.4))
        default:
            return nil
        }
    }

    static func summary(of input: InputSnapshot) -> String {
        var parts = ["phase=\(input.phase.rawValue)", "mode=\(input.mode.rawValue)",
                     "appearance=\(input.appearance.rawValue)", "backdrop=\(input.backdrop.tone.rawValue)",
                     "hover=\(input.hover)"]
        if let speech = input.speech {
            parts.append(String(format: "speech={level %.2f, progress %.2f}", speech.level, speech.progress))
        } else {
            parts.append("speech=null")
        }
        parts.append(String(format: "mic.level=%.2f", input.mic.level))
        if let show = input.show {
            let kinds = show.elements.map { element -> String in
                if case .text = element { return "text" }
                return "image"
            }
            parts.append("show=[\(kinds.joined(separator: ", "))]")
        } else {
            parts.append("show=null")
        }
        parts.append("ask=\(input.ask?.type.rawValue ?? "null")")
        parts.append("typing=\(input.typing.map { "\"\($0.text)\"" } ?? "null")")
        parts.append("slot=\(input.slot.index.rawValue)")
        return parts.joined(separator: ", ")
    }
}
