import AppKit
import Foundation
import OSLog
import PeekCore

/// `--simulate <scenario>`: present a Simulation scenario at launch, for automated screenshots.
///
/// ```
/// Peek.app/Contents/MacOS/Peek --simulate ask-single [--simulate-position 3] [--simulate-hold 120]
///     [--simulate-tone light|dark|live] [--simulate-appearance system|light|dark] [--simulate-mode normal|compact]
///     [--simulate-backdrop <image file>]   # Debug builds only
///     [--simulate-again <seconds>[:<scenario>]]   # once more (or another scenario) on the same, reused panel
/// ```
///
/// Scenarios are the ``SimulationPreset`` names (`show-text`, `show-cover`, `ask-single`,
/// `ask-multi`, `ask-slider`, `ask-range`, `ask-text`, `speak`, `compact-show`, the ui-feedback ones, and for
/// peek 0.1.2 `queue-badge` (a "+3" badge that updates to "+4" after 2 s), `ask-compact` (an ask collapsed by an Esc,
/// with `^` and a "+2" badge) and `ask-esc-hint` ("Esc again to dismiss" on the compact ask)). Shows stay up for
/// `--simulate-hold` seconds (default 600) instead of the usual 4–15 s, so a screenshot tool has
/// time; asks stay until answered. `--simulate-tone` picks the backdrop tone the pills are shaded for
/// (`live` samples the real desktop picture), `--simulate-appearance` the `input.appearance` the drawing
/// sees, `--simulate-mode` normal or compact. In Debug builds `--simulate-backdrop` covers the screen
/// (below the bubbles, above other apps) with an image, so screenshots show a known backdrop and nothing
/// else. Progress, errors and the bubble's panel rectangle (`screencapture -R` form) are written to
/// stderr and the unified log (subsystem `ai.tos.peek`, category `simulation`). An invalid request
/// opens the Simulation window with the error. Nothing is sent to peekd.
@MainActor
public enum SimulationLaunchHook {
    public struct Request: Equatable, Sendable {
        public var preset: SimulationPreset
        public var position: SlotIndex?
        public var holdSeconds: Int
        public var tone: SimulationScenario.BackdropChoice?
        public var appearance: SimulationScenario.AppearanceChoice?
        public var mode: DisplayMode?
        /// A backdrop image (Debug builds): shown full screen under the bubble.
        public var backdropImage: String?
        /// Present the scenario again this many seconds after the first one (the same panel, reused: slide-in captures
        /// of a later bubble).
        public var againSeconds: Int?
        /// The scenario presented the second time (default: the same one), e.g. another tint for the drawing.
        public var againPreset: SimulationPreset?

        public init(preset: SimulationPreset, position: SlotIndex? = nil, holdSeconds: Int = SimulationLaunchHook.defaultHoldSeconds,
                    tone: SimulationScenario.BackdropChoice? = nil, appearance: SimulationScenario.AppearanceChoice? = nil,
                    mode: DisplayMode? = nil, backdropImage: String? = nil, againSeconds: Int? = nil) {
            self.preset = preset
            self.position = position
            self.holdSeconds = holdSeconds
            self.tone = tone
            self.appearance = appearance
            self.mode = mode
            self.backdropImage = backdropImage
            self.againSeconds = againSeconds
        }

        /// The scenario to present: the preset with the overrides applied.
        public var scenario: SimulationScenario {
            var scenario = preset.scenario
            if let position { scenario.position = position }
            if let tone { scenario.backdropTone = tone }
            if let appearance { scenario.appearance = appearance }
            if let mode {
                scenario.mode = mode
                scenario.useSampleSpeech()
            }
            scenario.holdMs = holdSeconds * 1000
            return scenario
        }
    }

    public enum ParseError: Error, Equatable, Sendable, CustomStringConvertible {
        case missingScenario
        case unknownScenario(String)
        case invalidPosition(String)
        case invalidHold(String)
        case invalidAgain(String)
        case invalidChoice(flag: String, value: String, choices: [String])

        public var description: String {
            switch self {
            case .missingScenario:
                "--simulate needs a scenario name: --simulate <\(SimulationPreset.allCases.map(\.rawValue).joined(separator: "|"))>"
            case .unknownScenario(let name):
                "--simulate: there is no scenario \"\(name)\". Use one of: \(SimulationPreset.namesList)"
            case .invalidPosition(let raw):
                "--simulate-position must be a position from 1 (top centre) to 8 (top left), clockwise; got \"\(raw)\""
            case .invalidHold(let raw):
                "--simulate-hold must be a whole number of seconds from 1 to \(SimulationLaunchHook.maxHoldSeconds); got \"\(raw)\""
            case .invalidAgain(let raw):
                "--simulate-again must be <seconds>[:<scenario>] with 1 to \(SimulationLaunchHook.maxHoldSeconds) seconds and one "
                    + "of \(SimulationPreset.namesList); got \"\(raw)\""
            case .invalidChoice(let flag, let value, let choices):
                "\(flag) must be one of \(choices.joined(separator: ", ")); got \"\(value)\""
            }
        }
    }

    public nonisolated static let flag = "--simulate"
    public nonisolated static let positionFlag = "--simulate-position"
    public nonisolated static let holdFlag = "--simulate-hold"
    public nonisolated static let toneFlag = "--simulate-tone"
    public nonisolated static let appearanceFlag = "--simulate-appearance"
    public nonisolated static let modeFlag = "--simulate-mode"
    public nonisolated static let backdropFlag = "--simulate-backdrop"
    public nonisolated static let againFlag = "--simulate-again"
    public nonisolated static let defaultHoldSeconds = 600
    public nonisolated static let maxHoldSeconds = 86_400

    /// Parses the Simulation flags out of `arguments` (the executable first). `nil` when `--simulate` is absent.
    public nonisolated static func parse(_ arguments: [String]) -> Result<Request, ParseError>? {
        var name: String??
        var values: [String: String] = [:]
        let optionFlags = [positionFlag, holdFlag, toneFlag, appearanceFlag, modeFlag, backdropFlag, againFlag]
        var index = 1
        while index < arguments.count {
            let argument = arguments[index]
            func value(for flag: String) -> String? {
                if argument.hasPrefix(flag + "=") { return String(argument.dropFirst(flag.count + 1)) }
                guard index + 1 < arguments.count, !arguments[index + 1].hasPrefix("-") else { return nil }
                index += 1
                return arguments[index]
            }
            if argument == flag || argument.hasPrefix(flag + "=") {
                name = .some(value(for: flag))
            } else if let option = optionFlags.first(where: { argument == $0 || argument.hasPrefix($0 + "=") }) {
                values[option] = value(for: option) ?? ""
            }
            index += 1
        }
        guard let name else { return nil }
        guard let rawName = name, !rawName.isEmpty else { return .failure(.missingScenario) }
        guard let preset = SimulationPreset.named(rawName) else { return .failure(.unknownScenario(rawName)) }
        var request = Request(preset: preset)
        if let position = values[positionFlag] {
            guard let number = Int(position), let slot = SlotIndex(rawValue: number) else {
                return .failure(.invalidPosition(position))
            }
            request.position = slot
        }
        if let hold = values[holdFlag] {
            guard let seconds = Int(hold), (1...maxHoldSeconds).contains(seconds) else { return .failure(.invalidHold(hold)) }
            request.holdSeconds = seconds
        }
        if let raw = values[toneFlag] {
            guard let tone = SimulationScenario.BackdropChoice(rawValue: raw) else {
                return .failure(.invalidChoice(flag: toneFlag, value: raw,
                                               choices: SimulationScenario.BackdropChoice.allCases.map(\.rawValue)))
            }
            request.tone = tone
        }
        if let raw = values[appearanceFlag] {
            guard let appearance = SimulationScenario.AppearanceChoice(rawValue: raw) else {
                return .failure(.invalidChoice(flag: appearanceFlag, value: raw,
                                               choices: SimulationScenario.AppearanceChoice.allCases.map(\.rawValue)))
            }
            request.appearance = appearance
        }
        if let raw = values[modeFlag] {
            guard let mode = DisplayMode(rawValue: raw) else {
                return .failure(.invalidChoice(flag: modeFlag, value: raw, choices: DisplayMode.allCases.map(\.rawValue)))
            }
            request.mode = mode
        }
        if let raw = values[againFlag] {
            let parts = raw.split(separator: ":", maxSplits: 1).map(String.init)
            guard let first = parts.first, let seconds = Int(first), (1...maxHoldSeconds).contains(seconds) else {
                return .failure(.invalidAgain(raw))
            }
            request.againSeconds = seconds
            if parts.count == 2 {
                guard let preset = SimulationPreset.named(parts[1]) else { return .failure(.invalidAgain(raw)) }
                request.againPreset = preset
            }
        }
        if let raw = values[backdropFlag] {
            guard !raw.isEmpty else { return .failure(.invalidChoice(flag: backdropFlag, value: raw, choices: ["<image file>"])) }
            request.backdropImage = raw
        }
        return .success(request)
    }

    private static var scheduled = false
    private static let logger = PeekLogger(category: "simulation")

    /// Runs the requested scenario once, shortly after launch. Safe to call repeatedly: only the first call acts.
    public static func scheduleIfRequested(for coordinator: PeekCoordinator,
                                           arguments: [String] = CommandLine.arguments) {
        guard !scheduled, let parsed = parse(arguments) else { return }
        scheduled = true
        Task { @MainActor in
            // Let AppKit finish launching (screens, the status item) before a panel slides in.
            try? await Task.sleep(for: .milliseconds(600))
            await run(parsed, coordinator: coordinator)
        }
    }

    /// Slides any simulated bubble out and tears the Simulation presenter down (the app is quitting).
    public static func shutdown(for coordinator: PeekCoordinator) async {
        await SimulationEngine.existing(for: coordinator)?.shutdown()
    }

    static func run(_ parsed: Result<Request, ParseError>, coordinator: PeekCoordinator) async {
        let engine = SimulationEngine.shared(for: coordinator)
        switch parsed {
        case .failure(let error):
            report("peek simulation: \(error.description)", isError: true)
            engine.append(.error, error.description)
            SimulationWindowController.show(for: coordinator)
        case .success(let request):
            let previousEcho = engine.echo
            engine.echo = { entry in
                previousEcho?(entry)
                // The engine already logged the entry; only mirror it to stderr.
                report("peek simulation [\(entry.kind.rawValue)] \(entry.text)", isError: entry.kind == .error, log: false)
            }
            let scenario = request.scenario
            engine.scenario = scenario
            #if DEBUG
            if let image = request.backdropImage { SimulationBackdropWindow.show(imagePath: image) }
            #else
            if request.backdropImage != nil {
                report("peek simulation: \(backdropFlag) is only available in Debug builds; ignoring it", isError: false)
            }
            #endif
            report("peek simulation: presenting \(request.preset.rawValue) at position \(scenario.position.rawValue)",
                   isError: false)
            if await engine.simulate(scenario) {
                reportPanelFrame(for: scenario)
            }
            if let again = request.againSeconds {
                try? await Task.sleep(for: .seconds(again))
                var next = scenario
                if let preset = request.againPreset {
                    var other = Request(preset: preset, position: request.position, holdSeconds: request.holdSeconds,
                                        tone: request.tone, appearance: request.appearance, mode: request.mode)
                    other.position = scenario.position
                    next = other.scenario
                }
                report("peek simulation: presenting \((request.againPreset ?? request.preset).rawValue) (\(againFlag) \(again))",
                       isError: false)
                await engine.simulate(next)
            }
        }
    }

    /// Prints where the bubble's panel is, in `screencapture -R x,y,w,h` form (points, origin at the top left
    /// of the menu-bar screen), so screenshot tools can crop to the bubble.
    static func reportPanelFrame(for scenario: SimulationScenario) {
        let visible = ScreenPolicy.visibleFrame(for: .main)
        let layout = SlotGeometry.layout(slot: scenario.position, mode: scenario.mode, visibleFrame: visible)
        let frame = layout.panelFrame
        let primaryHeight = NSScreen.screens.first?.frame.height ?? visible.maxY
        let top = primaryHeight - frame.maxY
        let visual = layout.visualFrameOnScreen
        report(String(format: "peek simulation: panel %d,%d,%d,%d visual %d,%d,%d,%d (screencapture -R, points)",
                      Int(frame.minX), Int(top), Int(frame.width), Int(frame.height), Int(visual.minX),
                      Int(primaryHeight - visual.maxY), Int(visual.width), Int(visual.height)),
               isError: false)
    }

    private static func report(_ line: String, isError: Bool, log: Bool = true) {
        if log { if isError { logger.error("\(line)") } else { logger.notice("\(line)") } }
        FileHandle.standardError.write(Data((line + "\n").utf8))
    }
}
