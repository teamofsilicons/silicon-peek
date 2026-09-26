import Foundation

/// The `input` object a drawing receives every frame (visual.md A4, amended by BLUEPRINT §0.1):
///
/// * `speech` is `null | { text, level, progress, done }`: no `word`.
/// * `mic` is `{ level }`: no `transcript`.
/// * `phase` gains `transcribing`.
/// * new `context: 'production' | 'testing' | 'simulation'` and `glass: 'live' | 'frosted'`.
///
/// ``jsonBytes()`` is the hot path (up to 8 drawings × 120 Hz): a hand-written
/// writer with a fixed key order. `Codable` exists for tests, Simulation and dumps.
public struct InputSnapshot: Sendable, Equatable, Codable {
    public var t: Double
    public var dt: Double
    public var slot: Slot
    public var mode: DisplayMode
    public var appearance: Appearance
    public var backdrop: Backdrop
    public var phase: Phase
    public var hover: Bool
    public var mouse: Mouse
    public var speech: Speech?
    public var mic: Mic
    public var typing: Typing?
    public var show: Show?
    public var ask: Ask?
    public var context: InputContext
    public var glass: GlassMode

    public init(t: Double = 0, dt: Double = 0, slot: Slot, mode: DisplayMode = .normal, appearance: Appearance = .light,
                backdrop: Backdrop? = nil, phase: Phase = .hidden, hover: Bool = false, mouse: Mouse = .outside,
                speech: Speech? = nil, mic: Mic = Mic(level: 0), typing: Typing? = nil, show: Show? = nil,
                ask: Ask? = nil, context: InputContext = .production, glass: GlassMode = .live) {
        self.t = t
        self.dt = dt
        self.slot = slot
        self.mode = mode
        self.appearance = appearance
        self.backdrop = backdrop ?? .fromAppearance(appearance)
        self.phase = phase
        self.hover = hover
        self.mouse = mouse
        self.speech = speech
        self.mic = mic
        self.typing = typing
        self.show = show
        self.ask = ask
        self.context = context
        self.glass = glass
    }

    // MARK: Nested types

    public struct Slot: Sendable, Equatable, Codable {
        public var index: SlotIndex
        public var side: SlotSide
        /// Radians (y-down) from the bubble toward the screen centre.
        public var facing: Double

        public init(index: SlotIndex, facing: Double) {
            self.index = index
            self.side = index.side
            self.facing = facing
        }
    }

    public struct Mouse: Sendable, Equatable, Codable {
        public var x: Double
        public var y: Double
        /// Distance from (50, 50), in units.
        public var dist: Double
        /// Radians from (50, 50) to the pointer.
        public var angle: Double
        /// Inside the visual circle (r = 50).
        public var inside: Bool

        public init(x: Double, y: Double) {
            self.x = x
            self.y = y
            let dx = x - 50, dy = y - 50
            dist = (dx * dx + dy * dy).squareRoot()
            angle = atan2(dy, dx)
            inside = dist <= 50
        }

        /// A pointer far away from the visual (no hover).
        public static let outside = Mouse(x: -1000, y: -1000)
    }

    public struct Speech: Sendable, Equatable, Codable {
        public var text: String
        /// 0…1 loudness of the audio playing right now.
        public var level: Double
        /// 0…1 played frames / total frames.
        public var progress: Double
        public var done: Bool

        public init(text: String, level: Double, progress: Double, done: Bool) {
            self.text = text
            self.level = level
            self.progress = progress
            self.done = done
        }
    }

    public struct Mic: Sendable, Equatable, Codable {
        /// 0…1 loudness of the Carbon's microphone (0 when not listening).
        public var level: Double
        public init(level: Double) { self.level = level }
    }

    public struct Typing: Sendable, Equatable, Codable {
        public var text: String
        public init(text: String) { self.text = text }
    }

    public struct Show: Sendable, Equatable, Codable {
        public var elements: [Element]
        public init(elements: [Element]) { self.elements = elements }

        public enum Element: Sendable, Equatable, Codable {
            case text(String)
            case image(ImageHandle, caption: String?, colors: ImageColors)

            enum CodingKeys: String, CodingKey { case type, text, image, caption, colors }

            public init(from decoder: any Decoder) throws {
                let c = try decoder.container(keyedBy: CodingKeys.self)
                switch try c.decode(String.self, forKey: .type) {
                case "text": self = .text(try c.decode(String.self, forKey: .text))
                case "image":
                    self = .image(try c.decode(ImageHandle.self, forKey: .image),
                                  caption: try c.decodeIfPresent(String.self, forKey: .caption),
                                  colors: try c.decode(ImageColors.self, forKey: .colors))
                case let other:
                    throw DecodingError.dataCorruptedError(forKey: .type, in: c, debugDescription: "unknown element type \(other)")
                }
            }

            public func encode(to encoder: any Encoder) throws {
                var c = encoder.container(keyedBy: CodingKeys.self)
                switch self {
                case .text(let text):
                    try c.encode("text", forKey: .type)
                    try c.encode(text, forKey: .text)
                case .image(let handle, let caption, let colors):
                    try c.encode("image", forKey: .type)
                    try c.encode(handle, forKey: .image)
                    try c.encode(caption, forKey: .caption)
                    try c.encode(colors, forKey: .colors)
                }
            }
        }
    }

    public struct Ask: Sendable, Equatable, Codable {
        public var question: String
        public var type: AskType
        public var options: [Option]?
        public var min: Double?
        public var max: Double?
        public var step: Double?
        /// Live value: selection, slider value, range `[a, b]` or text. `nil` → `null`.
        public var value: AskValue?
        /// Option id the pointer is over (no voice matching: BLUEPRINT §0.1 item 1).
        public var highlight: String?

        public init(question: String, type: AskType, options: [Option]? = nil, min: Double? = nil, max: Double? = nil,
                    step: Double? = nil, value: AskValue? = nil, highlight: String? = nil) {
            self.question = question
            self.type = type
            self.options = options
            self.min = min
            self.max = max
            self.step = step
            self.value = value
            self.highlight = highlight
        }

        public struct Option: Sendable, Equatable, Codable {
            public var id: String
            public var label: String
            public var image: ImageHandle?
            public var colors: ImageColors?

            public init(id: String, label: String, image: ImageHandle? = nil, colors: ImageColors? = nil) {
                self.id = id
                self.label = label
                self.image = image
                self.colors = colors
            }

            public func encode(to encoder: any Encoder) throws {
                var c = encoder.container(keyedBy: CodingKeys.self)
                try c.encode(id, forKey: .id)
                try c.encode(label, forKey: .label)
                try c.encode(image, forKey: .image)
                try c.encode(colors, forKey: .colors)
            }
        }

        enum CodingKeys: String, CodingKey { case question, type, options, min, max, step, value, highlight }

        public init(from decoder: any Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            question = try c.decode(String.self, forKey: .question)
            type = try c.decode(AskType.self, forKey: .type)
            options = try c.decodeIfPresent([Option].self, forKey: .options)
            min = try c.decodeIfPresent(Double.self, forKey: .min)
            max = try c.decodeIfPresent(Double.self, forKey: .max)
            step = try c.decodeIfPresent(Double.self, forKey: .step)
            let raw = try c.decodeIfPresent(JSONValue.self, forKey: .value) ?? .null
            value = AskValue(json: raw, for: type)
            highlight = try c.decodeIfPresent(String.self, forKey: .highlight)
        }

        public func encode(to encoder: any Encoder) throws {
            var c = encoder.container(keyedBy: CodingKeys.self)
            try c.encode(question, forKey: .question)
            try c.encode(type, forKey: .type)
            try c.encodeIfPresent(options, forKey: .options)
            try c.encodeIfPresent(min, forKey: .min)
            try c.encodeIfPresent(max, forKey: .max)
            try c.encodeIfPresent(step, forKey: .step)
            try c.encode(value?.jsonValue ?? .null, forKey: .value)
            try c.encode(highlight, forKey: .highlight)
        }

        /// Builds the drawing's view of an ask; `images` maps option image paths to prepared handles.
        public init(_ payload: AskPayload, value: AskValue?, highlight: String?, images: [String: PreparedImage] = [:]) {
            question = payload.question
            type = payload.type
            self.value = value
            self.highlight = highlight
            switch payload.kind {
            case .text:
                options = nil
            case .singleChoice(let list), .multipleChoice(let list, _, _):
                options = list.map { option in
                    let prepared = option.image.flatMap { images[$0] }
                    return Option(id: option.id, label: option.label, image: prepared?.handle, colors: prepared?.colors)
                }
            case .slider(let spec):
                options = nil
                min = spec.min
                max = spec.max
                step = spec.step
            case .range(let spec):
                options = nil
                min = spec.min
                max = spec.max
                step = spec.step
            }
        }
    }

    // MARK: JSON for injection

    /// Compact JSON for `peek_vm_frame`. Keys are always present (optional values are `null`).
    public func jsonBytes() -> [UInt8] {
        var out: [UInt8] = []
        out.reserveCapacity(768)
        writeJSON(into: &out)
        return out
    }

    public func writeJSON(into out: inout [UInt8]) {
        var w = FieldWriter(&out)
        w.number("t", t, &out)
        w.number("dt", dt, &out)
        w.key("slot", &out)
        do {
            var s = FieldWriter(&out)
            s.number("index", Double(slot.index.rawValue), &out)
            s.string("side", slot.side.rawValue, &out)
            s.number("facing", slot.facing, &out)
            s.close(&out)
        }
        w.string("mode", mode.rawValue, &out)
        w.string("appearance", appearance.rawValue, &out)
        w.key("backdrop", &out)
        backdrop.writeJSON(into: &out)
        w.string("phase", phase.rawValue, &out)
        w.bool("hover", hover, &out)
        w.key("mouse", &out)
        do {
            var m = FieldWriter(&out)
            m.number("x", mouse.x, &out)
            m.number("y", mouse.y, &out)
            m.number("dist", mouse.dist, &out)
            m.number("angle", mouse.angle, &out)
            m.bool("inside", mouse.inside, &out)
            m.close(&out)
        }
        w.key("speech", &out)
        if let speech {
            var s = FieldWriter(&out)
            s.string("text", speech.text, &out)
            s.number("level", speech.level, &out)
            s.number("progress", speech.progress, &out)
            s.bool("done", speech.done, &out)
            s.close(&out)
        } else {
            JSONWriter.writeNull(into: &out)
        }
        w.key("mic", &out)
        do {
            var m = FieldWriter(&out)
            m.number("level", mic.level, &out)
            m.close(&out)
        }
        w.key("typing", &out)
        if let typing {
            var t = FieldWriter(&out)
            t.string("text", typing.text, &out)
            t.close(&out)
        } else {
            JSONWriter.writeNull(into: &out)
        }
        w.key("show", &out)
        if let show {
            var s = FieldWriter(&out)
            s.key("elements", &out)
            out.append(UInt8(ascii: "["))
            for (index, element) in show.elements.enumerated() {
                if index > 0 { out.append(UInt8(ascii: ",")) }
                var e = FieldWriter(&out)
                switch element {
                case .text(let text):
                    e.string("type", "text", &out)
                    e.string("text", text, &out)
                case .image(let handle, let caption, let colors):
                    e.string("type", "image", &out)
                    e.key("image", &out)
                    handle.writeJSON(into: &out)
                    e.optionalString("caption", caption, &out)
                    e.key("colors", &out)
                    colors.writeJSON(into: &out)
                }
                e.close(&out)
            }
            out.append(UInt8(ascii: "]"))
            s.close(&out)
        } else {
            JSONWriter.writeNull(into: &out)
        }
        w.key("ask", &out)
        if let ask {
            var a = FieldWriter(&out)
            a.string("question", ask.question, &out)
            a.string("type", ask.type.rawValue, &out)
            if let options = ask.options {
                a.key("options", &out)
                out.append(UInt8(ascii: "["))
                for (index, option) in options.enumerated() {
                    if index > 0 { out.append(UInt8(ascii: ",")) }
                    var o = FieldWriter(&out)
                    o.string("id", option.id, &out)
                    o.string("label", option.label, &out)
                    o.key("image", &out)
                    if let image = option.image { image.writeJSON(into: &out) } else { JSONWriter.writeNull(into: &out) }
                    o.key("colors", &out)
                    if let colors = option.colors { colors.writeJSON(into: &out) } else { JSONWriter.writeNull(into: &out) }
                    o.close(&out)
                }
                out.append(UInt8(ascii: "]"))
            }
            if let min = ask.min { a.number("min", min, &out) }
            if let max = ask.max { a.number("max", max, &out) }
            if let step = ask.step { a.number("step", step, &out) }
            a.key("value", &out)
            (ask.value?.jsonValue ?? .null).write(into: &out)
            a.optionalString("highlight", ask.highlight, &out)
            a.close(&out)
        } else {
            JSONWriter.writeNull(into: &out)
        }
        w.string("context", context.rawValue, &out)
        w.string("glass", glass.rawValue, &out)
        w.close(&out)
    }
}

/// An opaque image reference for `ctx.drawImage` (visual.md A4, BLUEPRINT §8.6: `{id, width, height}`).
/// Handles become invalid when the next send replaces the content.
public struct ImageHandle: Sendable, Hashable, Codable {
    public var id: Int
    public var width: Int
    public var height: Int

    public init(id: Int, width: Int, height: Int) {
        self.id = id
        self.width = width
        self.height = height
    }

    func writeJSON(into out: inout [UInt8]) {
        var w = FieldWriter(&out)
        w.number("id", Double(id), &out)
        w.number("width", Double(width), &out)
        w.number("height", Double(height), &out)
        w.close(&out)
    }
}

/// `colors` of an image: dominant colour plus a 3–5 colour palette (visual.md B7), as `#rrggbb`.
public struct ImageColors: Sendable, Hashable, Codable {
    public var dominant: String
    public var palette: [String]

    public init(dominant: String, palette: [String]) {
        self.dominant = dominant
        self.palette = palette
    }

    func writeJSON(into out: inout [UInt8]) {
        var w = FieldWriter(&out)
        w.string("dominant", dominant, &out)
        w.key("palette", &out)
        out.append(UInt8(ascii: "["))
        for (index, color) in palette.enumerated() {
            if index > 0 { out.append(UInt8(ascii: ",")) }
            JSONWriter.writeString(color, into: &out)
        }
        out.append(UInt8(ascii: "]"))
        w.close(&out)
    }
}

/// An image copied, decoded (≤ 512 px) and analysed for one send.
public struct PreparedImage: Sendable, Hashable {
    public var handle: ImageHandle
    public var colors: ImageColors

    public init(handle: ImageHandle, colors: ImageColors) {
        self.handle = handle
        self.colors = colors
    }
}

public enum BackdropTone: String, Codable, Sendable, CaseIterable {
    case light
    case dark

    /// Hysteresis at 0.45 / 0.55 (B8, §8.5): the tone only flips once luminance clearly crosses.
    public static func next(previous: BackdropTone?, luminance: Double) -> BackdropTone {
        switch previous {
        case .light?: luminance < 0.45 ? .dark : .light
        case .dark?: luminance > 0.55 ? .light : .dark
        case nil: luminance < 0.5 ? .dark : .light
        }
    }
}

public enum BackdropSource: String, Codable, Sendable, CaseIterable {
    case screen
    case wallpaper
    case appearance
}

/// `input.backdrop`: peek's estimate of what the bubble sits on (B8).
public struct Backdrop: Sendable, Equatable, Codable {
    public var tone: BackdropTone
    /// 0 (black) … 1 (white), Rec. 709 on linear sRGB.
    public var luminance: Double
    /// Average colour, `#rrggbb`.
    public var color: String
    /// The high-contrast ink: the opposite of `tone`.
    public var ink: String
    public var source: BackdropSource

    public init(tone: BackdropTone, luminance: Double, color: String, source: BackdropSource) {
        self.tone = tone
        self.luminance = luminance
        self.color = color
        self.ink = tone == .dark ? "#ffffff" : "#000000"
        self.source = source
    }

    /// A sampled colour (sRGB components 0…1), applying hysteresis against the previous tone.
    public static func sample(red: Double, green: Double, blue: Double, source: BackdropSource,
                              previousTone: BackdropTone?) -> Backdrop {
        let luminance = relativeLuminance(red: red, green: green, blue: blue)
        return Backdrop(tone: BackdropTone.next(previous: previousTone, luminance: luminance), luminance: luminance,
                        color: hexColor(red: red, green: green, blue: blue), source: source)
    }

    /// The last-resort estimate: dark mode is a dark backdrop, light mode a light one.
    public static func fromAppearance(_ appearance: Appearance) -> Backdrop {
        switch appearance {
        case .dark: Backdrop(tone: .dark, luminance: 0.1, color: "#1e1e1e", source: .appearance)
        case .light: Backdrop(tone: .light, luminance: 0.9, color: "#ececec", source: .appearance)
        }
    }

    /// Rec. 709 relative luminance of an sRGB colour (components 0…1, gamma-encoded).
    public static func relativeLuminance(red: Double, green: Double, blue: Double) -> Double {
        func linear(_ c: Double) -> Double {
            let c = min(max(c, 0), 1)
            return c <= 0.04045 ? c / 12.92 : pow((c + 0.055) / 1.055, 2.4)
        }
        return 0.2126 * linear(red) + 0.7152 * linear(green) + 0.0722 * linear(blue)
    }

    public static func hexColor(red: Double, green: Double, blue: Double) -> String {
        func byte(_ c: Double) -> Int { Int((min(max(c, 0), 1) * 255).rounded()) }
        return String(format: "#%02x%02x%02x", byte(red), byte(green), byte(blue))
    }

    func writeJSON(into out: inout [UInt8]) {
        var w = FieldWriter(&out)
        w.string("tone", tone.rawValue, &out)
        w.number("luminance", luminance, &out)
        w.string("color", color, &out)
        w.string("ink", ink, &out)
        w.string("source", source.rawValue, &out)
        w.close(&out)
    }
}

/// Writes one JSON object field by field into a caller-owned buffer (keys are trusted ASCII literals).
struct FieldWriter {
    private var first = true

    init(_ out: inout [UInt8]) { out.append(UInt8(ascii: "{")) }

    mutating func key(_ name: StaticString, _ out: inout [UInt8]) {
        if !first { out.append(UInt8(ascii: ",")) }
        first = false
        out.append(UInt8(ascii: "\""))
        name.withUTF8Buffer { out.append(contentsOf: $0) }
        out.append(UInt8(ascii: "\""))
        out.append(UInt8(ascii: ":"))
    }

    mutating func number(_ name: StaticString, _ value: Double, _ out: inout [UInt8]) {
        key(name, &out)
        JSONWriter.writeNumber(value, into: &out)
    }

    mutating func string(_ name: StaticString, _ value: String, _ out: inout [UInt8]) {
        key(name, &out)
        JSONWriter.writeString(value, into: &out)
    }

    mutating func optionalString(_ name: StaticString, _ value: String?, _ out: inout [UInt8]) {
        key(name, &out)
        if let value { JSONWriter.writeString(value, into: &out) } else { JSONWriter.writeNull(into: &out) }
    }

    mutating func bool(_ name: StaticString, _ value: Bool, _ out: inout [UInt8]) {
        key(name, &out)
        JSONWriter.writeBool(value, into: &out)
    }

    func close(_ out: inout [UInt8]) { out.append(UInt8(ascii: "}")) }
}
