import Foundation
import PeekCore

/// Everything the Simulation toggles control (BLUEPRINT §8.11): where the bubble appears, what it
/// speaks, shows or asks, and the environment the drawing sees (mode, appearance, backdrop tone,
/// context). ``makeEvent(assets:sendID:askID:)`` turns it into the same `peek.show` event peekd sends.
public struct SimulationScenario: Equatable, Sendable {
    public enum Content: String, CaseIterable, Sendable {
        case none
        case show
        case ask
    }

    public enum ShowVariant: String, CaseIterable, Sendable {
        /// One text pill.
        case text
        /// One image with a caption.
        case imageCaption = "image"
        /// Image + caption, then two text pills: the maximum of 3 elements (the DJ case).
        case threeElements = "three"
        /// One long text: it wraps into a narrow, tall column (ui-feedback.md #4).
        case longText = "long"
        /// A cover with a long caption (cut short: hover reveals it, a click expands it), a long text and a short one.
        case longMixed = "long-mixed"

        public var title: String {
            switch self {
            case .text: "Text"
            case .imageCaption: "Image + caption"
            case .threeElements: "3 elements"
            case .longText: "Long text"
            case .longMixed: "Long text + long caption"
            }
        }
    }

    /// Which sample texts and values an ask uses.
    public enum AskSample: String, CaseIterable, Sendable {
        case standard
        /// A long question and long option labels, cut short on the arc (hover reveals them).
        case long
        /// A slider with a handful of discrete steps, each marked with a dot (ui-feedback.md #6).
        case stepped

        public var title: String {
            switch self {
            case .standard: "Standard"
            case .long: "Long labels"
            case .stepped: "Stepped slider"
            }
        }
    }

    public enum AppearanceChoice: String, CaseIterable, Sendable {
        case system
        case light
        case dark
    }

    public enum BackdropChoice: String, CaseIterable, Sendable {
        case light
        case dark
        /// Sample the real desktop under the bubble (the app's live backdrop sampler).
        case live
    }

    public var position: SlotIndex = .top
    public var speak = false
    public var speakText = SimulationSamples.speakOnly
    public var content: Content = .show
    public var showVariant: ShowVariant = .text
    public var askType: AskType = .singleChoice
    /// Give single- and multiple-choice options images.
    public var optionImages = true
    public var askSample: AskSample = .standard
    public var mode: DisplayMode = .normal
    public var appearance: AppearanceChoice = .system
    public var backdropTone: BackdropChoice = .light
    /// What the drawing sees as `input.context`. The bubble itself is always a Simulation bubble.
    public var context: InputContext = .simulation
    /// `duration_ms` for shows; nil = the §7.4 default (3 s + 0.06 s per visible character, 4…15 s).
    public var holdMs: Int?

    public init() {}

    /// Replaces the speak text with the sample matching the current content.
    public mutating func useSampleSpeech() {
        speakText = SimulationSamples.speakText(for: self)
    }

    /// The `peek.show` event for this scenario.
    public func makeEvent(assets: SimulationAssetPaths, sendID: String, askID: String) throws(SimulationScenarioError)
        -> PeekShowEvent
    {
        var speakInfo: SpeakInfo?
        if speak {
            let text = speakText.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !text.isEmpty else { throw .emptySpeech }
            guard text.scalarCount <= Self.maxSpeakScalars else { throw .speechTooLong(text.scalarCount) }
            speakInfo = SpeakInfo(text: text, status: .pending)
        }
        var show: ShowPayload?
        var ask: AskPayload?
        switch content {
        case .none:
            break
        case .show:
            let payload = SimulationSamples.show(showVariant, assets: assets)
            do throws(PayloadError) { try payload.validate() } catch { throw .invalidPayload(error) }
            show = payload
        case .ask:
            let payload = SimulationSamples.ask(askType, optionImages: optionImages, sample: askSample, assets: assets)
            do throws(PayloadError) { try payload.validate() } catch { throw .invalidPayload(error) }
            ask = payload
        }
        guard speakInfo != nil || show != nil || ask != nil else { throw .nothingToShow }
        return PeekShowEvent(
            sendID: sendID, askID: ask == nil ? nil : askID, slot: position, context: .simulation, speak: speakInfo,
            show: show, ask: ask, durationMs: ask == nil ? holdMs : nil)
    }

    /// Checks the scenario without files on disk.
    public func validate() throws(SimulationScenarioError) {
        _ = try makeEvent(assets: .placeholder, sendID: "snd_validate", askID: "ask_validate")
    }

    /// Aura-2's per-request limit, which `peek send --speak` enforces (BLUEPRINT §0.1 item 8).
    public static let maxSpeakScalars = 2000

    /// A fresh `snd_…` / `ask_…` id in the D25 shape (prefix + 32 hex).
    public static func newID(_ prefix: String) -> String {
        prefix + "_" + UUID().uuidString.replacingOccurrences(of: "-", with: "").lowercased()
    }
}

public enum SimulationScenarioError: Error, Equatable, Sendable, CustomStringConvertible {
    case nothingToShow
    case emptySpeech
    case speechTooLong(Int)
    case invalidPayload(PayloadError)

    public var description: String {
        switch self {
        case .nothingToShow:
            "Nothing to simulate: turn on Speak, or choose Show or Ask. A real `peek send` needs at least one of --speak, --show or --ask."
        case .emptySpeech:
            "Speak is on but the text is empty. Type something to say, or turn Speak off."
        case .speechTooLong(let count):
            "The speak text has \(count) characters; Deepgram Aura-2 speaks at most \(SimulationScenario.maxSpeakScalars) per send."
        case .invalidPayload(let error):
            "The sample payload is invalid (\(error.code)): \(error.message)"
        }
    }
}

/// The named scenarios: the Simulation window's presets and the `--simulate <name>` launch argument.
public enum SimulationPreset: String, CaseIterable, Sendable, Identifiable {
    case showText = "show-text"
    case showCover = "show-cover"
    case askSingle = "ask-single"
    case askMulti = "ask-multi"
    case askSlider = "ask-slider"
    case askRange = "ask-range"
    case askText = "ask-text"
    case speak = "speak"
    case compactShow = "compact-show"
    // ui-feedback.md scenarios (2026-09-26).
    case showLong = "show-long"
    case showLongMixed = "show-long-mixed"
    case askLongLabels = "ask-long-labels"
    case askStepped = "ask-stepped"
    case compactLong = "compact-long"

    public var id: String { rawValue }

    public var title: String {
        switch self {
        case .showText: "Show text"
        case .showCover: "Show cover art (DJ)"
        case .askSingle: "Ask: single choice with images"
        case .askMulti: "Ask: multiple choice"
        case .askSlider: "Ask: slider"
        case .askRange: "Ask: range"
        case .askText: "Ask: text"
        case .speak: "Speak only"
        case .compactShow: "Compact mode show"
        case .showLong: "Show long text (narrow and tall)"
        case .showLongMixed: "Show long text + long caption"
        case .askLongLabels: "Ask: long question and labels"
        case .askStepped: "Ask: stepped slider"
        case .compactLong: "Compact mode long text"
        }
    }

    public var scenario: SimulationScenario {
        var scenario = SimulationScenario()
        switch self {
        case .showText:
            scenario.content = .show
            scenario.showVariant = .text
        case .showCover:
            scenario.content = .show
            scenario.showVariant = .threeElements
            scenario.speak = true
        case .askSingle:
            scenario.content = .ask
            scenario.askType = .singleChoice
            scenario.optionImages = true
            scenario.speak = true
        case .askMulti:
            scenario.content = .ask
            scenario.askType = .multipleChoice
            scenario.optionImages = false
            scenario.speak = true
        case .askSlider:
            scenario.content = .ask
            scenario.askType = .slider
        case .askRange:
            scenario.content = .ask
            scenario.askType = .range
        case .askText:
            scenario.content = .ask
            scenario.askType = .text
            scenario.speak = true
        case .speak:
            scenario.content = .none
            scenario.speak = true
        case .compactShow:
            scenario.content = .show
            scenario.showVariant = .text
            scenario.mode = .compact
        case .showLong:
            scenario.content = .show
            scenario.showVariant = .longText
        case .showLongMixed:
            scenario.content = .show
            scenario.showVariant = .longMixed
        case .askLongLabels:
            scenario.content = .ask
            scenario.askType = .singleChoice
            scenario.optionImages = false
            scenario.askSample = .long
        case .askStepped:
            scenario.content = .ask
            scenario.askType = .slider
            scenario.askSample = .stepped
        case .compactLong:
            scenario.content = .show
            scenario.showVariant = .longText
            scenario.mode = .compact
        }
        scenario.useSampleSpeech()
        return scenario
    }

    public static func named(_ name: String) -> SimulationPreset? {
        SimulationPreset(rawValue: name.trimmingCharacters(in: .whitespaces).lowercased())
    }

    public static var namesList: String { allCases.map(\.rawValue).joined(separator: ", ") }
}

/// The sample values Simulation uses. Every name, file and title is fictional.
public enum SimulationSamples {
    public static let speakOnly = "Heads up: the nightly backup finished. Twelve gigabytes copied, and no errors."
    public static let showText = "Standup in 5 minutes, room Orion. Bring the Q3 numbers."
    public static let compactText = "Time for some water. It's been two hours."
    /// A long show text (≤ 160 characters): it runs narrow and tall.
    public static let longText =
        "Your 3 pm with the design team moved to Thursday at 10 in room Atlas. Bring the revised onboarding flow and the latest retention numbers."
    /// A caption too long for its card (≤ 50 characters).
    public static let longCaption = "Neon Tide · Late Night Sessions (Deluxe Edition)"
    public static let longQuestion = "Three old exports are taking 14 GB in Downloads. What should I do with them?"

    public static func speakText(for scenario: SimulationScenario) -> String {
        switch scenario.content {
        case .none:
            return speakOnly
        case .show:
            if scenario.mode == .compact, scenario.showVariant == .text { return compactText }
            if scenario.mode == .compact, scenario.showVariant == .longText { return "Your design review moved to Thursday." }
            switch scenario.showVariant {
            case .text: return "Standup starts in five minutes, in room Orion."
            case .longText: return "Heads up: your design review moved to Thursday at ten."
            case .imageCaption, .threeElements, .longMixed:
                let cover = SimulationArtwork.Cover.neonTide
                return "Now playing \(cover.title) by \(cover.artist), from \(cover.year)."
            }
        case .ask:
            if scenario.askSample == .long, scenario.askType != .text {
                return "Three old exports are filling up your Downloads folder. What should I do with them?"
            }
            switch scenario.askType {
            case .text: return "What should I call tonight's playlist?"
            case .singleChoice: return "Budget final v3 hasn't been opened in nine months. Should I keep it?"
            case .multipleChoice: return "Which moods fit tonight's set? Pick as many as you like."
            case .slider:
                return scenario.askSample == .stepped ? "Out of ten, how was tonight's set?" : "How loud should the music be?"
            case .range: return "When can I book focus time today?"
            }
        }
    }

    public static func show(_ variant: SimulationScenario.ShowVariant, assets: SimulationAssetPaths) -> ShowPayload {
        let cover = SimulationArtwork.Cover.neonTide
        switch variant {
        case .text:
            return ShowPayload(elements: [.text(showText)])
        case .imageCaption:
            return ShowPayload(elements: [.image(path: assets.cover(cover), caption: cover.title)])
        case .threeElements:
            return ShowPayload(elements: [
                .image(path: assets.cover(cover), caption: cover.title), .text(cover.artist), .text(cover.year),
            ])
        case .longText:
            return ShowPayload(elements: [.text(longText)])
        case .longMixed:
            return ShowPayload(elements: [
                .image(path: assets.cover(cover), caption: longCaption), .text(longText), .text(cover.year),
            ])
        }
    }

    public static func ask(_ type: AskType, optionImages: Bool, sample: SimulationScenario.AskSample = .standard,
                           assets: SimulationAssetPaths) -> AskPayload {
        func image(_ icon: SimulationArtwork.Icon) -> String? { optionImages ? assets.icon(icon) : nil }
        switch (type, sample) {
        case (.singleChoice, .long), (.multipleChoice, .long):
            let options = [
                AskOption(id: "archive", label: "Move all three to the archive drive", image: image(.archive)),
                AskOption(id: "delete", label: "Delete them permanently right now", image: image(.delete)),
                AskOption(id: "later", label: "Keep them and remind me next week", image: image(.keep)),
            ]
            return AskPayload(
                question: longQuestion,
                kind: type == .singleChoice ? .singleChoice(options: options) : .multipleChoice(options: options, min: 1, max: 3))
        case (.text, .long):
            return AskPayload(
                question: "What should I call the playlist for tonight's long drive up the coast?",
                kind: .text(placeholder: "e.g. Coastline at midnight, windows down", maxLength: 60))
        case (.slider, .stepped):
            return AskPayload(
                question: "Out of ten, how was tonight's set?",
                kind: .slider(SliderSpec(min: 0, max: 10, step: 1, defaultValue: 7, unit: nil)))
        default:
            break
        }
        switch type {
        case .text:
            return AskPayload(
                question: "What should I call tonight's playlist?", kind: .text(placeholder: "e.g. Late-night focus", maxLength: 60))
        case .singleChoice:
            return AskPayload(
                question: "budget-final-v3.xlsx hasn't been opened in 9 months. Keep it?",
                kind: .singleChoice(options: [
                    AskOption(id: "keep", label: "Keep", image: image(.keep)),
                    AskOption(id: "archive", label: "Archive", image: image(.archive)),
                    AskOption(id: "delete", label: "Delete", image: image(.delete)),
                ]))
        case .multipleChoice:
            let options = [
                AskOption(id: "calm", label: "Calm", image: image(.calm)),
                AskOption(id: "upbeat", label: "Upbeat", image: image(.upbeat)),
                AskOption(id: "dreamy", label: "Dreamy", image: image(.dreamy)),
                AskOption(id: "after-dark", label: "After dark", image: image(.dark)),
            ]
            return AskPayload(
                question: "Which moods fit tonight's set?", kind: .multipleChoice(options: options, min: 1, max: options.count))
        case .slider:
            return AskPayload(
                question: "How loud should the music be?",
                kind: .slider(SliderSpec(min: 0, max: 100, step: 5, defaultValue: 40, unit: "%")))
        case .range:
            return AskPayload(
                question: "When can I book focus time today?",
                kind: .range(RangeSpec(min: 6, max: 22, step: 1, defaultValue: (9, 12), unit: "h")))
        }
    }
}
