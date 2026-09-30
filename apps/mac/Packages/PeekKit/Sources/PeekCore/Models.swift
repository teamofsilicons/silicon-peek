import Foundation

// MARK: - Slots

/// One of the 8 positions: 1 is top centre, then clockwise to 8 at top-left (understanding.md, visual.md A4).
public struct SlotIndex: RawRepresentable, Hashable, Comparable, Sendable, Codable, CaseIterable,
    CustomStringConvertible
{
    public let rawValue: Int

    public init?(rawValue: Int) {
        guard (1...8).contains(rawValue) else { return nil }
        self.rawValue = rawValue
    }

    private init(unchecked value: Int) { rawValue = value }

    public static let top = SlotIndex(unchecked: 1)
    public static let topRight = SlotIndex(unchecked: 2)
    public static let right = SlotIndex(unchecked: 3)
    public static let bottomRight = SlotIndex(unchecked: 4)
    public static let bottom = SlotIndex(unchecked: 5)
    public static let bottomLeft = SlotIndex(unchecked: 6)
    public static let left = SlotIndex(unchecked: 7)
    public static let topLeft = SlotIndex(unchecked: 8)

    public static let allCases: [SlotIndex] = (1...8).map(SlotIndex.init(unchecked:))

    public var side: SlotSide { SlotSide.allCases[rawValue - 1] }

    /// Carbon virtual key code of the digit key (physical ANSI position): kVK_ANSI_1…8.
    public var hotKeyCode: UInt32 { Self.keyCodes[rawValue - 1] }
    private static let keyCodes: [UInt32] = [0x12, 0x13, 0x14, 0x15, 0x17, 0x16, 0x1A, 0x1C]

    public static func < (lhs: SlotIndex, rhs: SlotIndex) -> Bool { lhs.rawValue < rhs.rawValue }

    public var description: String { "\(rawValue) (\(side.rawValue))" }

    public init(from decoder: any Decoder) throws {
        let container = try decoder.singleValueContainer()
        let value = try container.decode(Int.self)
        guard let slot = SlotIndex(rawValue: value) else {
            throw DecodingError.dataCorruptedError(
                in: container, debugDescription: "slot index \(value) is outside 1...8")
        }
        self = slot
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(rawValue)
    }
}

/// The side names used by `input.slot.side` and `peek register side` output.
public enum SlotSide: String, Codable, Sendable, CaseIterable {
    case top
    case topRight = "top-right"
    case right
    case bottomRight = "bottom-right"
    case bottom
    case bottomLeft = "bottom-left"
    case left
    case topLeft = "top-left"

    public var index: SlotIndex { SlotIndex.allCases[SlotSide.allCases.firstIndex(of: self)!] }

    /// Unit vector from the slot toward the screen centre in compass terms
    /// (y-down): the direction the information arc faces (peek all-positions.jpg).
    public var inward: (dx: Double, dy: Double) {
        let d = 1 / 2.0.squareRoot()
        switch self {
        case .top: return (0, 1)
        case .topRight: return (-d, d)
        case .right: return (-1, 0)
        case .bottomRight: return (-d, -d)
        case .bottom: return (0, -1)
        case .bottomLeft: return (d, -d)
        case .left: return (1, 0)
        case .topLeft: return (d, d)
        }
    }

    public var isCorner: Bool {
        switch self {
        case .topRight, .bottomRight, .bottomLeft, .topLeft: true
        default: false
        }
    }

    /// True for the left and right slots, whose arc runs vertically.
    public var isVertical: Bool { self == .left || self == .right }
}

/// The modifier combination for the per-slot hotkeys (§8.5). The default is `ctrl+cmd` (ctrl+cmd+1…8):
/// plain ⌘1…8 collides with tab switching in browsers, editors and terminals. It stays configurable.
public enum HotkeyModifier: String, Codable, Sendable, CaseIterable {
    case cmd = "cmd"
    case ctrlCmd = "ctrl+cmd"
    case optCmd = "opt+cmd"
    case shiftCmd = "shift+cmd"
    case ctrlOptCmd = "ctrl+opt+cmd"
    case ctrlOpt = "ctrl+opt"

    // Carbon modifier masks (Events.h): cmdKey 1<<8, shiftKey 1<<9, optionKey 1<<11, controlKey 1<<12.
    public static let carbonCmd: UInt32 = 1 << 8
    public static let carbonShift: UInt32 = 1 << 9
    public static let carbonOption: UInt32 = 1 << 11
    public static let carbonControl: UInt32 = 1 << 12

    /// Modifier mask for `RegisterEventHotKey`.
    public var carbonMask: UInt32 {
        switch self {
        case .cmd: Self.carbonCmd
        case .ctrlCmd: Self.carbonControl | Self.carbonCmd
        case .optCmd: Self.carbonOption | Self.carbonCmd
        case .shiftCmd: Self.carbonShift | Self.carbonCmd
        case .ctrlOptCmd: Self.carbonControl | Self.carbonOption | Self.carbonCmd
        case .ctrlOpt: Self.carbonControl | Self.carbonOption
        }
    }

    /// Menu-style glyphs, e.g. "⌃⌘".
    public var symbols: String {
        switch self {
        case .cmd: "⌘"
        case .ctrlCmd: "⌃⌘"
        case .optCmd: "⌥⌘"
        case .shiftCmd: "⇧⌘"
        case .ctrlOptCmd: "⌃⌥⌘"
        case .ctrlOpt: "⌃⌥"
        }
    }

    /// The textual hotkey, e.g. `cmd+5` (the `register side` output format, §7.4).
    public func label(for slot: SlotIndex) -> String { "\(rawValue)+\(slot.rawValue)" }
}

// MARK: - Contexts and identity

/// Which world a slot, bubble or drawing belongs to. On the wire it is
/// `"production"` or a testing environment UUID; `"simulation"` is local-only.
public enum PeekContext: Hashable, Sendable, Codable, CustomStringConvertible {
    case production
    case testing(environmentID: String)
    case simulation

    public init(rawValue: String) {
        switch rawValue {
        case "production": self = .production
        case "simulation": self = .simulation
        default: self = .testing(environmentID: rawValue)
        }
    }

    public var rawValue: String {
        switch self {
        case .production: "production"
        case .simulation: "simulation"
        case .testing(let id): id
        }
    }

    /// The value drawings see as `input.context`.
    public var inputContext: InputContext {
        switch self {
        case .production: .production
        case .testing: .testing
        case .simulation: .simulation
        }
    }

    public var description: String { rawValue }

    public init(from decoder: any Decoder) throws {
        let container = try decoder.singleValueContainer()
        let raw = try container.decode(String.self)
        guard !raw.isEmpty else {
            throw DecodingError.dataCorruptedError(in: container, debugDescription: "context must not be empty")
        }
        self.init(rawValue: raw)
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(rawValue)
    }
}

/// `input.context` (BLUEPRINT §0.1 item 4).
public enum InputContext: String, Codable, Sendable, CaseIterable {
    case production
    case testing
    case simulation
}

/// Identifies one Silicon's drawing and bubble within a context. Drawings are keyed by this,
/// not by slot, because the same script instance survives `register side` moves (visual.md A1).
public struct SiliconKey: Hashable, Sendable, Codable, CustomStringConvertible {
    public var context: PeekContext
    public var orgID: String
    public var actorID: String

    public init(context: PeekContext, orgID: String, actorID: String) {
        self.context = context
        self.orgID = orgID
        self.actorID = actorID
    }

    public var description: String { "\(actorID)[\(orgID)]@\(context.rawValue)" }

    enum CodingKeys: String, CodingKey {
        case context
        case orgID = "org_id"
        case actorID = "actor_id"
    }
}

// MARK: - Display settings enums

public enum DisplayMode: String, Codable, Sendable, CaseIterable {
    case normal
    case compact
}

public enum DisplayTarget: String, Codable, Sendable, CaseIterable {
    /// The menu-bar screen (`NSScreen.screens[0]`).
    case main
    /// The screen under the pointer at send time.
    case pointer
}

public enum BackdropSourceSetting: String, Codable, Sendable, CaseIterable {
    case wallpaper
    /// ScreenCaptureKit sampling; opt-in because Tahoe re-prompts periodically (§8.8).
    case screen
}

public enum Appearance: String, Codable, Sendable, CaseIterable {
    case light
    case dark
}

/// `input.glass` (BLUEPRINT §0.1 item 5): `frosted` only when the private active-appearance override is unavailable.
public enum GlassMode: String, Codable, Sendable, CaseIterable {
    case live
    case frosted
}

/// `input.phase` (visual.md A4 + `transcribing`, BLUEPRINT §0.1 item 3).
public enum Phase: String, Codable, Sendable, CaseIterable {
    case hidden
    case entering
    case showing
    case asking
    case speaking
    case listening
    case typing
    case transcribing
    case leaving
}

// MARK: - Payload validation

/// A payload that breaks a §7.4 limit. `code` uses the CLI's error codes.
public struct PayloadError: Error, Equatable, Sendable, CustomStringConvertible {
    public var code: String
    public var message: String

    public init(_ code: String, _ message: String) {
        self.code = code
        self.message = message
    }

    public var description: String { "\(code): \(message)" }
}

extension String {
    /// Length in Unicode scalar values, the unit every peek limit is measured in (§7.4).
    public var scalarCount: Int { unicodeScalars.count }
}

// MARK: - Show

/// `--show` after peekd copied the images into its cache (§1.6 `peek.show`).
public struct ShowPayload: Codable, Sendable, Equatable {
    public static let maxElements = 3
    public static let maxTextScalars = 160
    public static let maxCaptionScalars = 50

    public var elements: [ShowElement]

    public init(elements: [ShowElement]) { self.elements = elements }

    /// Every image path, in element order.
    public var imagePaths: [String] {
        elements.compactMap { element in
            if case .image(let path, _) = element { return path }
            return nil
        }
    }

    public func validate() throws(PayloadError) {
        guard !elements.isEmpty else { throw PayloadError("invalid_input", "show needs at least 1 element") }
        guard elements.count <= Self.maxElements else {
            throw PayloadError("too_many_elements", "show has \(elements.count) elements; at most \(Self.maxElements) fit on the arc")
        }
        for (index, element) in elements.enumerated() {
            switch element {
            case .text(let text):
                guard (1...Self.maxTextScalars).contains(text.scalarCount) else {
                    throw PayloadError(
                        "text_too_long",
                        "show element \(index) text has \(text.scalarCount) characters; it must be 1...\(Self.maxTextScalars)")
                }
            case .image(let path, let caption):
                guard !path.isEmpty else { throw PayloadError("invalid_input", "show element \(index) has an empty image path") }
                if let caption, caption.scalarCount > Self.maxCaptionScalars {
                    throw PayloadError(
                        "caption_too_long",
                        "show element \(index) caption has \(caption.scalarCount) characters; at most \(Self.maxCaptionScalars)")
                }
            }
        }
    }
}

public enum ShowElement: Codable, Sendable, Equatable {
    case text(String)
    /// `path` is peekd's cache path (`cache/images/<sha256>.<ext>`), never the Silicon's own path.
    case image(path: String, caption: String?)

    enum CodingKeys: String, CodingKey { case type, text, path, caption }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        let type = try c.decode(String.self, forKey: .type)
        switch type {
        case "text":
            self = .text(try c.decode(String.self, forKey: .text))
        case "image":
            self = .image(path: try c.decode(String.self, forKey: .path),
                          caption: try c.decodeIfPresent(String.self, forKey: .caption))
        default:
            throw DecodingError.dataCorruptedError(
                forKey: .type, in: c, debugDescription: "show element type \"\(type)\" is not text or image")
        }
    }

    public func encode(to encoder: any Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .text(let text):
            try c.encode("text", forKey: .type)
            try c.encode(text, forKey: .text)
        case .image(let path, let caption):
            try c.encode("image", forKey: .type)
            try c.encode(path, forKey: .path)
            try c.encode(caption, forKey: .caption)
        }
    }
}

// MARK: - Ask

public enum AskType: String, Codable, Sendable, CaseIterable {
    case text
    case singleChoice = "single_choice"
    case multipleChoice = "multiple_choice"
    case slider
    case range
}

/// One choice. `id` defaults to "1", "2", … in order when the Silicon gave none (§7.4).
public struct AskOption: Codable, Sendable, Equatable, Identifiable {
    public var id: String
    public var label: String
    /// peekd's cache path of the option image, if any.
    public var image: String?

    public init(id: String, label: String, image: String? = nil) {
        self.id = id
        self.label = label
        self.image = image
    }

    enum CodingKeys: String, CodingKey { case id, label, image }

    public func encode(to encoder: any Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(id, forKey: .id)
        try c.encode(label, forKey: .label)
        try c.encodeIfPresent(image, forKey: .image)
    }
}

/// One option as it may appear on the wire: `"Label"` shorthand or an object whose id may be missing.
private struct AskOptionWire: Decodable {
    var id: String?
    var label: String
    var image: String?

    enum CodingKeys: String, CodingKey { case id, label, image }
    enum ImageKeys: String, CodingKey { case path }

    init(from decoder: any Decoder) throws {
        if let single = try? decoder.singleValueContainer(), let label = try? single.decode(String.self) {
            self.label = label
            return
        }
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decodeIfPresent(String.self, forKey: .id)
        label = try c.decode(String.self, forKey: .label)
        if let path = try? c.decodeIfPresent(String.self, forKey: .image) {
            image = path
        } else if c.contains(.image), let nested = try? c.nestedContainer(keyedBy: ImageKeys.self, forKey: .image) {
            image = try nested.decode(String.self, forKey: .path)
        }
    }
}

public struct SliderSpec: Sendable, Equatable {
    public var min: Double
    public var max: Double
    public var step: Double
    public var defaultValue: Double
    public var unit: String?

    public init(min: Double, max: Double, step: Double? = nil, defaultValue: Double? = nil, unit: String? = nil) {
        self.min = min
        self.max = max
        self.step = step ?? (max - min) / 100
        self.defaultValue = defaultValue ?? min
        self.unit = unit
    }
}

public struct RangeSpec: Sendable, Equatable {
    public var min: Double
    public var max: Double
    public var step: Double
    public var defaultLower: Double
    public var defaultUpper: Double
    public var unit: String?

    public init(min: Double, max: Double, step: Double? = nil, defaultValue: (Double, Double)? = nil,
                unit: String? = nil) {
        self.min = min
        self.max = max
        self.step = step ?? (max - min) / 100
        self.defaultLower = defaultValue?.0 ?? min
        self.defaultUpper = defaultValue?.1 ?? max
        self.unit = unit
    }
}

public enum AskKind: Sendable, Equatable {
    case text(placeholder: String?, maxLength: Int)
    case singleChoice(options: [AskOption])
    case multipleChoice(options: [AskOption], min: Int, max: Int)
    case slider(SliderSpec)
    case range(RangeSpec)

    public var type: AskType {
        switch self {
        case .text: .text
        case .singleChoice: .singleChoice
        case .multipleChoice: .multipleChoice
        case .slider: .slider
        case .range: .range
        }
    }

    public var options: [AskOption] {
        switch self {
        case .singleChoice(let options), .multipleChoice(let options, _, _): options
        default: []
        }
    }
}

/// `--ask` (§7.4), normalized: option ids and every default are filled in.
public struct AskPayload: Codable, Sendable, Equatable {
    public static let maxQuestionScalars = 80
    public static let defaultTextMaxLength = 500
    public static let optionCountRange = 2...6
    public static let maxOptionLabelScalars = 40

    public var question: String
    public var kind: AskKind

    public init(question: String, kind: AskKind) {
        self.question = question
        self.kind = kind
    }

    public var type: AskType { kind.type }
    public var options: [AskOption] { kind.options }
    public var imagePaths: [String] { options.compactMap(\.image) }

    enum CodingKeys: String, CodingKey {
        case question, type, placeholder, options, min, max, step, unit
        case maxLength = "max_length"
        case defaultValue = "default"
    }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        question = try c.decode(String.self, forKey: .question)
        let type = try c.decode(AskType.self, forKey: .type)
        func options() throws -> [AskOption] {
            let wire = try c.decode([AskOptionWire].self, forKey: .options)
            return wire.enumerated().map { index, option in
                AskOption(id: option.id ?? String(index + 1), label: option.label, image: option.image)
            }
        }
        switch type {
        case .text:
            kind = .text(
                placeholder: try c.decodeIfPresent(String.self, forKey: .placeholder),
                maxLength: try c.decodeIfPresent(Int.self, forKey: .maxLength) ?? Self.defaultTextMaxLength)
        case .singleChoice:
            kind = .singleChoice(options: try options())
        case .multipleChoice:
            let list = try options()
            kind = .multipleChoice(
                options: list,
                min: try c.decodeIfPresent(Int.self, forKey: .min) ?? 1,
                max: try c.decodeIfPresent(Int.self, forKey: .max) ?? list.count)
        case .slider:
            kind = .slider(
                SliderSpec(
                    min: try c.decode(Double.self, forKey: .min),
                    max: try c.decode(Double.self, forKey: .max),
                    step: try c.decodeIfPresent(Double.self, forKey: .step),
                    defaultValue: try c.decodeIfPresent(Double.self, forKey: .defaultValue),
                    unit: try c.decodeIfPresent(String.self, forKey: .unit)))
        case .range:
            let pair = try c.decodeIfPresent([Double].self, forKey: .defaultValue)
            if let pair, pair.count != 2 {
                throw DecodingError.dataCorruptedError(
                    forKey: .defaultValue, in: c, debugDescription: "range default must be [from, to]")
            }
            kind = .range(
                RangeSpec(
                    min: try c.decode(Double.self, forKey: .min),
                    max: try c.decode(Double.self, forKey: .max),
                    step: try c.decodeIfPresent(Double.self, forKey: .step),
                    defaultValue: pair.map { ($0[0], $0[1]) },
                    unit: try c.decodeIfPresent(String.self, forKey: .unit)))
        }
    }

    public func encode(to encoder: any Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(question, forKey: .question)
        try c.encode(type, forKey: .type)
        switch kind {
        case .text(let placeholder, let maxLength):
            try c.encodeIfPresent(placeholder, forKey: .placeholder)
            try c.encode(maxLength, forKey: .maxLength)
        case .singleChoice(let options):
            try c.encode(options, forKey: .options)
        case .multipleChoice(let options, let min, let max):
            try c.encode(options, forKey: .options)
            try c.encode(min, forKey: .min)
            try c.encode(max, forKey: .max)
        case .slider(let spec):
            try c.encode(spec.min, forKey: .min)
            try c.encode(spec.max, forKey: .max)
            try c.encode(spec.step, forKey: .step)
            try c.encode(spec.defaultValue, forKey: .defaultValue)
            try c.encodeIfPresent(spec.unit, forKey: .unit)
        case .range(let spec):
            try c.encode(spec.min, forKey: .min)
            try c.encode(spec.max, forKey: .max)
            try c.encode(spec.step, forKey: .step)
            try c.encode([spec.defaultLower, spec.defaultUpper], forKey: .defaultValue)
            try c.encodeIfPresent(spec.unit, forKey: .unit)
        }
    }

    /// Checks every §7.4 limit. peekd validated the payload already; the UI re-checks
    /// defensively and Simulation uses this for its own samples.
    public func validate() throws(PayloadError) {
        guard (1...Self.maxQuestionScalars).contains(question.scalarCount) else {
            throw PayloadError(
                "question_too_long",
                "ask question has \(question.scalarCount) characters; it must be 1...\(Self.maxQuestionScalars)")
        }
        switch kind {
        case .text(let placeholder, let maxLength):
            if let placeholder, placeholder.scalarCount > 60 {
                throw PayloadError("invalid_input", "ask placeholder has \(placeholder.scalarCount) characters; at most 60")
            }
            guard (1...2000).contains(maxLength) else {
                throw PayloadError("invalid_input", "ask max_length \(maxLength) must be 1...2000")
            }
        case .singleChoice(let options):
            try Self.validate(options)
        case .multipleChoice(let options, let min, let max):
            try Self.validate(options)
            guard min >= 0, max >= 1, min <= max, max <= options.count else {
                throw PayloadError(
                    "invalid_input",
                    "multiple_choice needs 0 <= min <= max <= \(options.count) and max >= 1; got min \(min), max \(max)")
            }
        case .slider(let spec):
            try Self.validateBounds(min: spec.min, max: spec.max, step: spec.step, unit: spec.unit)
            guard (spec.min...spec.max).contains(spec.defaultValue) else {
                throw PayloadError("invalid_input", "slider default \(spec.defaultValue) is outside \(spec.min)...\(spec.max)")
            }
        case .range(let spec):
            try Self.validateBounds(min: spec.min, max: spec.max, step: spec.step, unit: spec.unit)
            guard spec.defaultLower <= spec.defaultUpper, (spec.min...spec.max).contains(spec.defaultLower),
                (spec.min...spec.max).contains(spec.defaultUpper)
            else {
                throw PayloadError(
                    "invalid_input",
                    "range default [\(spec.defaultLower), \(spec.defaultUpper)] must be ordered and inside \(spec.min)...\(spec.max)")
            }
        }
    }

    private static func validate(_ options: [AskOption]) throws(PayloadError) {
        guard optionCountRange.contains(options.count) else {
            throw PayloadError("too_many_options", "ask has \(options.count) options; it must have 2...6")
        }
        var seen = Set<String>()
        for option in options {
            guard Self.isValidOptionID(option.id) else {
                throw PayloadError("invalid_input", "option id \"\(option.id)\" must match ^[a-z0-9_-]{1,32}$")
            }
            guard seen.insert(option.id).inserted else {
                throw PayloadError("invalid_input", "option id \"\(option.id)\" is used twice")
            }
            guard (1...maxOptionLabelScalars).contains(option.label.scalarCount) else {
                throw PayloadError(
                    "invalid_input",
                    "option \"\(option.id)\" label has \(option.label.scalarCount) characters; it must be 1...\(maxOptionLabelScalars)")
            }
        }
    }

    private static func validateBounds(min: Double, max: Double, step: Double, unit: String?) throws(PayloadError) {
        guard min.isFinite, max.isFinite, max > min else {
            throw PayloadError("invalid_input", "max (\(max)) must be greater than min (\(min))")
        }
        guard step.isFinite, step > 0 else { throw PayloadError("invalid_input", "step \(step) must be > 0") }
        if let unit, unit.scalarCount > 8 {
            throw PayloadError("invalid_input", "unit \"\(unit)\" has \(unit.scalarCount) characters; at most 8")
        }
    }

    public static func isValidOptionID(_ id: String) -> Bool {
        let bytes = Array(id.utf8)
        guard (1...32).contains(bytes.count) else { return false }
        return bytes.allSatisfy { byte in
            (UInt8(ascii: "a")...UInt8(ascii: "z")).contains(byte) || (UInt8(ascii: "0")...UInt8(ascii: "9")).contains(byte)
                || byte == UInt8(ascii: "_") || byte == UInt8(ascii: "-")
        }
    }
}

/// The value of an ask: the live `input.ask.value` and the `answer.value` sent to peekd.
///
/// On the wire it is raw JSON: text → string, single choice → option id, multiple
/// choice → array of option ids, slider → number, range → `[from, to]`.
public enum AskValue: Sendable, Hashable {
    case text(String)
    case choice(String)
    case choices([String])
    case number(Double)
    case range(lower: Double, upper: Double)

    public var jsonValue: JSONValue {
        switch self {
        case .text(let text): .string(text)
        case .choice(let id): .string(id)
        case .choices(let ids): .array(ids.map(JSONValue.string))
        case .number(let value): .double(value)
        case .range(let lower, let upper): .array([.double(lower), .double(upper)])
        }
    }

    /// Interprets a raw value for an ask of `type` (e.g. `stt.result.value`).
    public init?(json: JSONValue, for type: AskType) {
        switch (type, json) {
        case (.text, .string(let text)): self = .text(text)
        case (.singleChoice, .string(let id)): self = .choice(id)
        case (.multipleChoice, .array(let items)):
            let ids = items.compactMap(\.stringValue)
            guard ids.count == items.count else { return nil }
            self = .choices(ids)
        case (.slider, let value):
            guard let number = value.doubleValue else { return nil }
            self = .number(number)
        case (.range, .array(let items)):
            guard items.count == 2, let lower = items[0].doubleValue, let upper = items[1].doubleValue else { return nil }
            self = .range(lower: lower, upper: upper)
        default:
            return nil
        }
    }

    /// The value an ask starts with before the Carbon touches it (`nil` = nothing selected yet).
    public static func initial(for ask: AskPayload) -> AskValue? {
        switch ask.kind {
        case .text: .text("")
        case .singleChoice: nil
        case .multipleChoice: .choices([])
        case .slider(let spec): .number(spec.defaultValue)
        case .range(let spec): .range(lower: spec.defaultLower, upper: spec.defaultUpper)
        }
    }
}

// MARK: - peek.show

/// TTS status peekd reported for a send (§7.4 `speech.status`). Unknown values decode as ``other(_:)``.
public enum SpeechStatus: Sendable, Hashable, Codable, CustomStringConvertible {
    case pending
    case cached
    case skipped
    case unsupportedLanguage
    case other(String)

    public init(rawValue: String) {
        switch rawValue {
        case "pending": self = .pending
        case "cached": self = .cached
        case "skipped": self = .skipped
        case "unsupported_language": self = .unsupportedLanguage
        default: self = .other(rawValue)
        }
    }

    public var rawValue: String {
        switch self {
        case .pending: "pending"
        case .cached: "cached"
        case .skipped: "skipped"
        case .unsupportedLanguage: "unsupported_language"
        case .other(let value): value
        }
    }

    /// Whether PCM will arrive (`tts.begin` …). Otherwise the text is shown as a pill (§1.9.3).
    public var expectsAudio: Bool { self == .pending || self == .cached }

    public var description: String { rawValue }
    public init(from decoder: any Decoder) throws { self.init(rawValue: try decoder.singleValueContainer().decode(String.self)) }
    public func encode(to encoder: any Encoder) throws {
        var c = encoder.singleValueContainer()
        try c.encode(rawValue)
    }
}

public struct SpeakInfo: Codable, Sendable, Equatable {
    public var text: String
    public var status: SpeechStatus

    public init(text: String, status: SpeechStatus) {
        self.text = text
        self.status = status
    }
}

/// `peek.show`: slide a bubble in (§1.6, §1.9.3).
public struct PeekShowEvent: Codable, Sendable, Equatable {
    public static let name = "peek.show"

    public var sendID: String
    /// The ask to answer. Read from the top-level `ask_id`, falling back to `ask.ask_id`.
    public var askID: String?
    public var slot: SlotIndex
    public var context: PeekContext
    public var speak: SpeakInfo?
    public var show: ShowPayload?
    public var ask: AskPayload?
    /// How long a show stays up without speech, or after it (§7.4 `--duration`). `nil` = the default formula.
    public var durationMs: Int?
    /// Sends of this Silicon waiting behind this one (peek 0.1.2: waiting + due-waiting): the "+N" badge's first value.
    public var queuedBehind: Int
    /// When the send expires (RFC 3339), for every kind since peek 0.1.2 (asks only before). Informational: peekd
    /// withdraws an expired bubble with `peek.cancel{reason: "expired"}`.
    public var expiresAt: String?
    /// `--replace`: the send this one takes over (Peek.app already got `peek.cancel{reason: "replaced"}` for it).
    public var replaces: String?
    /// `sch_…` when the send came from `peek send --in/--at`.
    public var scheduleID: String?

    public init(sendID: String, askID: String? = nil, slot: SlotIndex, context: PeekContext = .production,
                speak: SpeakInfo? = nil, show: ShowPayload? = nil, ask: AskPayload? = nil, durationMs: Int? = nil,
                queuedBehind: Int = 0, expiresAt: String? = nil, replaces: String? = nil, scheduleID: String? = nil) {
        self.sendID = sendID
        self.askID = askID
        self.slot = slot
        self.context = context
        self.speak = speak
        self.show = show
        self.ask = ask
        self.durationMs = durationMs
        self.queuedBehind = queuedBehind
        self.expiresAt = expiresAt
        self.replaces = replaces
        self.scheduleID = scheduleID
    }

    enum CodingKeys: String, CodingKey {
        case sendID = "send_id"
        case askID = "ask_id"
        case slot, context, speak, show, ask
        case durationMs = "duration_ms"
        case queuedBehind = "queued_behind"
        case expiresAt = "expires_at"
        case replaces
        case scheduleID = "schedule_id"
    }

    private enum NestedAskKeys: String, CodingKey { case askID = "ask_id" }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        sendID = try c.decode(String.self, forKey: .sendID)
        slot = try c.decode(SlotIndex.self, forKey: .slot)
        context = try c.decodeIfPresent(PeekContext.self, forKey: .context) ?? .production
        speak = try c.decodeIfPresent(SpeakInfo.self, forKey: .speak)
        show = try c.decodeIfPresent(ShowPayload.self, forKey: .show)
        ask = try c.decodeIfPresent(AskPayload.self, forKey: .ask)
        durationMs = try c.decodeIfPresent(Int.self, forKey: .durationMs)
        queuedBehind = max(0, try c.decodeIfPresent(Int.self, forKey: .queuedBehind) ?? 0)
        expiresAt = try c.decodeIfPresent(String.self, forKey: .expiresAt)
        replaces = try c.decodeIfPresent(String.self, forKey: .replaces)
        scheduleID = try c.decodeIfPresent(String.self, forKey: .scheduleID)
        if let top = try c.decodeIfPresent(String.self, forKey: .askID) {
            askID = top
        } else if (try? c.decodeNil(forKey: .ask)) == false,
            let nested = try? c.nestedContainer(keyedBy: NestedAskKeys.self, forKey: .ask)
        {
            askID = try nested.decodeIfPresent(String.self, forKey: .askID)
        } else {
            askID = nil
        }
    }

    public func encode(to encoder: any Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(sendID, forKey: .sendID)
        try c.encode(askID, forKey: .askID)
        try c.encode(slot, forKey: .slot)
        try c.encode(context, forKey: .context)
        try c.encode(speak, forKey: .speak)
        try c.encode(show, forKey: .show)
        try c.encode(ask, forKey: .ask)
        try c.encode(durationMs, forKey: .durationMs)
        try c.encode(queuedBehind, forKey: .queuedBehind)
        try c.encodeIfPresent(expiresAt, forKey: .expiresAt)
        try c.encodeIfPresent(replaces, forKey: .replaces)
        try c.encodeIfPresent(scheduleID, forKey: .scheduleID)
    }

    /// Visible characters of the show (text and captions), used by the default duration.
    public var visibleCharacterCount: Int {
        (show?.elements ?? []).reduce(0) { total, element in
            switch element {
            case .text(let text): total + text.scalarCount
            case .image(_, let caption): total + (caption?.scalarCount ?? 0)
            }
        }
    }

    /// `--duration`, or the §7.4 default `clamp(3 + 0.06 × visible characters, 4, 15)` seconds.
    public var effectiveDuration: Duration {
        if let durationMs { return .milliseconds(durationMs) }
        let seconds = min(max(3 + 0.06 * Double(visibleCharacterCount), 4), 15)
        return .milliseconds(Int((seconds * 1000).rounded()))
    }
}

// MARK: - slots.state

public struct DrawingRef: Codable, Sendable, Equatable {
    public var sha256: String
    public var path: String

    public init(sha256: String, path: String) {
        self.sha256 = sha256
        self.path = path
    }
}

/// Testing-environment label for the `TEST · <name>` pill and its tooltip (gap-testing §9.3).
/// Additive: not in the §1.6 table yet.
public struct TestEnvironmentInfo: Codable, Sendable, Equatable {
    public var id: String?
    public var name: String
    public var generation: Int?

    public init(id: String? = nil, name: String, generation: Int? = nil) {
        self.id = id
        self.name = name
        self.generation = generation
    }
}

/// One registered slot in `slots.state`. Production and testing contexts may hold the same index.
public struct SlotState: Codable, Sendable, Equatable, Identifiable {
    public var index: SlotIndex
    public var context: PeekContext
    public var actorID: String
    public var orgID: String
    public var displayName: String
    /// One grapheme for the fallback visual (visual.md A7).
    public var initial: String
    public var drawing: DrawingRef?
    /// Whether the UI should register this slot's hotkey.
    public var hotkey: Bool
    public var environment: TestEnvironmentInfo?

    public init(index: SlotIndex, context: PeekContext = .production, actorID: String, orgID: String,
                displayName: String? = nil, initial: String? = nil, drawing: DrawingRef? = nil, hotkey: Bool = true,
                environment: TestEnvironmentInfo? = nil) {
        self.index = index
        self.context = context
        self.actorID = actorID
        self.orgID = orgID
        let name = displayName ?? Self.defaultDisplayName(actorID)
        self.displayName = name
        self.initial = initial ?? Self.defaultInitial(name)
        self.drawing = drawing
        self.hotkey = hotkey
        self.environment = environment
    }

    public var id: String { "\(context.rawValue)#\(index.rawValue)" }
    public var siliconKey: SiliconKey { SiliconKey(context: context, orgID: orgID, actorID: actorID) }

    enum CodingKeys: String, CodingKey {
        case index, context, drawing, hotkey, initial, environment
        case actorID = "actor_id"
        case orgID = "org_id"
        case displayName = "display_name"
    }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        index = try c.decode(SlotIndex.self, forKey: .index)
        context = try c.decodeIfPresent(PeekContext.self, forKey: .context) ?? .production
        actorID = try c.decode(String.self, forKey: .actorID)
        orgID = try c.decode(String.self, forKey: .orgID)
        let name = try c.decodeIfPresent(String.self, forKey: .displayName).flatMap { $0.isEmpty ? nil : $0 }
            ?? Self.defaultDisplayName(actorID)
        displayName = name
        initial = try c.decodeIfPresent(String.self, forKey: .initial).flatMap { $0.isEmpty ? nil : $0 }
            ?? Self.defaultInitial(name)
        drawing = try c.decodeIfPresent(DrawingRef.self, forKey: .drawing)
        hotkey = try c.decodeIfPresent(Bool.self, forKey: .hotkey) ?? true
        environment = try c.decodeIfPresent(TestEnvironmentInfo.self, forKey: .environment)
    }

    public func encode(to encoder: any Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(index, forKey: .index)
        try c.encode(context, forKey: .context)
        try c.encode(actorID, forKey: .actorID)
        try c.encode(orgID, forKey: .orgID)
        try c.encode(displayName, forKey: .displayName)
        try c.encode(initial, forKey: .initial)
        try c.encode(drawing, forKey: .drawing)
        try c.encode(hotkey, forKey: .hotkey)
        try c.encodeIfPresent(environment, forKey: .environment)
    }

    /// `si:dj` → `dj`.
    static func defaultDisplayName(_ actorID: String) -> String {
        guard let colon = actorID.firstIndex(of: ":") else { return actorID }
        let handle = actorID[actorID.index(after: colon)...]
        return handle.isEmpty ? actorID : String(handle)
    }

    static func defaultInitial(_ name: String) -> String {
        guard let first = name.first(where: { $0.isLetter || $0.isNumber }) ?? name.first else { return "?" }
        return String(first).uppercased()
    }
}

/// A Silicon moved from one slot to another between two `slots.state` tables (drives the drawing's `move` event).
public struct SlotMove: Sendable, Equatable {
    public var key: SiliconKey
    public var from: SlotIndex
    public var to: SlotIndex

    public init(key: SiliconKey, from: SlotIndex, to: SlotIndex) {
        self.key = key
        self.from = from
        self.to = to
    }

    /// Moves implied by the change from `old` to `new` (same Silicon, different index).
    public static func between(_ old: [SlotState], _ new: [SlotState]) -> [SlotMove] {
        let before = Dictionary(old.map { ($0.siliconKey, $0.index) }, uniquingKeysWith: { first, _ in first })
        return new.compactMap { state in
            guard let previous = before[state.siliconKey], previous != state.index else { return nil }
            return SlotMove(key: state.siliconKey, from: previous, to: state.index)
        }
        .sorted { $0.to < $1.to }
    }
}

// MARK: - Voices

/// Common language preferences shown in Settings; ElevenLabs voices are multilingual.
public enum DefaultVoices {
    public static let supportedLanguages = ["en", "es", "de", "fr", "nl", "it", "ja", "hi", "pt", "zh", "ko", "ru", "ar"]
    public static let byLanguage = Dictionary(uniqueKeysWithValues: supportedLanguages.map { ($0, "JBFqnCBsd6RMkjVDRZzb") })

    /// ElevenLabs voice IDs; the provider checks availability.
    public static func isValidVoice(_ voice: String) -> Bool {
        (1...128).contains(voice.utf8.count) && voice.utf8.allSatisfy {
            (UInt8(ascii: "a")...UInt8(ascii: "z")).contains($0)
                || (UInt8(ascii: "A")...UInt8(ascii: "Z")).contains($0)
                || (UInt8(ascii: "0")...UInt8(ascii: "9")).contains($0)
                || $0 == UInt8(ascii: "_") || $0 == UInt8(ascii: "-")
        }
    }
}

// MARK: - Launch arguments

/// Arguments Peek.app understands (§1.8, gap-honeycomb §4.2):
/// * `--after-update <old build>` after a self-update swap;
/// * `--launched-by <who>` when the CLI opened it (`open -g -j Peek.app --args --launched-by cli`);
/// * `--uninstall` (`peek app uninstall`'s fallback): unregister the services, recycle the bundle, quit;
/// * the Simulation flags (`--simulate <scenario>` and the `--simulate-*` options), parsed in full by
///   PeekUI's `SimulationLaunchHook`; they are only recognised here so they are not reported as unknown.
public struct AppLaunchArguments: Sendable, Equatable {
    /// Simulation flags that take a value.
    public static let simulationValueFlags = [
        "--simulate", "--simulate-position", "--simulate-hold", "--simulate-tone", "--simulate-appearance",
        "--simulate-mode", "--simulate-backdrop",
    ]
    public static let uninstallFlag = "--uninstall"

    public var afterUpdateFromBuild: Int?
    public var launchedBy: String?
    /// `--uninstall` was passed.
    public var uninstall: Bool
    /// The Simulation flags as given (flag and value), for logging.
    public var simulation: [String]
    /// Arguments this build does not know (AppKit's own `-NSDocumentRevisionsDebugMode` etc. are skipped).
    public var unrecognized: [String]

    public init(afterUpdateFromBuild: Int? = nil, launchedBy: String? = nil, uninstall: Bool = false,
                simulation: [String] = [], unrecognized: [String] = []) {
        self.afterUpdateFromBuild = afterUpdateFromBuild
        self.launchedBy = launchedBy
        self.uninstall = uninstall
        self.simulation = simulation
        self.unrecognized = unrecognized
    }

    public var isAfterUpdate: Bool { afterUpdateFromBuild != nil }
    /// `--launched-by cli`.
    public var isLaunchedByCLI: Bool { launchedBy == "cli" }
    /// `--simulate` is present (valid or not).
    public var wantsSimulation: Bool { simulation.contains { $0 == "--simulate" || $0.hasPrefix("--simulate=") } }

    /// Parses `CommandLine.arguments` (the first element, the executable, is skipped).
    public static func parse(_ arguments: [String]) -> AppLaunchArguments {
        var result = AppLaunchArguments()
        var index = 1
        func value(after flag: String, at i: Int) -> (String?, Int) {
            if let eq = arguments[i].firstIndex(of: "=") {
                return (String(arguments[i][arguments[i].index(after: eq)...]), i + 1)
            }
            guard i + 1 < arguments.count, !arguments[i + 1].hasPrefix("--") else { return (nil, i + 1) }
            return (arguments[i + 1], i + 2)
        }
        func matches(_ argument: String, _ flag: String) -> Bool { argument == flag || argument.hasPrefix(flag + "=") }
        while index < arguments.count {
            let argument = arguments[index]
            if matches(argument, "--after-update") {
                let (raw, next) = value(after: "--after-update", at: index)
                result.afterUpdateFromBuild = raw.flatMap(Int.init) ?? 0
                index = next
            } else if matches(argument, "--launched-by") {
                let (raw, next) = value(after: "--launched-by", at: index)
                result.launchedBy = raw
                index = next
            } else if argument == uninstallFlag {
                result.uninstall = true
                index += 1
            } else if let flag = simulationValueFlags.first(where: { matches(argument, $0) }) {
                let (raw, next) = value(after: flag, at: index)
                result.simulation.append(argument)
                if !argument.contains("="), let raw { result.simulation.append(raw) }
                index = next
            } else if argument.hasPrefix("-NS") || argument.hasPrefix("-Apple") {
                // AppKit/Xcode defaults arguments come in pairs: "-Key value".
                index += (index + 1 < arguments.count && !arguments[index + 1].hasPrefix("-")) ? 2 : 1
            } else {
                result.unrecognized.append(argument)
                index += 1
            }
        }
        return result
    }
}
