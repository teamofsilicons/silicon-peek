import Foundation
import PeekCore

/// Simulation's input feed: the real ``InputHubbing`` (speech level and progress from the sample
/// PCM, pointer, typing, images) with the Simulation toggles laid over it: the appearance and the
/// `input.context` the drawing sees. The backdrop tone comes from ``SimulatedBackdrop``, which the
/// wrapped hub reads like any backdrop sampler.
@MainActor
public final class SimulationInputHub: InputHubbing {
    public let base: any InputHubbing
    /// Forced `input.appearance` (and the chrome's), or nil to follow the system.
    public private(set) var appearanceOverride: Appearance?
    /// The `input.context` every simulated drawing sees.
    public private(set) var contextOverride: InputContext = .simulation
    /// Called after every update the presenter makes, with the resulting state (phase for the window).
    public var onStateChange: (@MainActor (SiliconKey, BubbleInputState) -> Void)?

    private var sources: [SiliconKey: SimulationInputSource] = [:]

    public init(base: any InputHubbing) {
        self.base = base
    }

    public func setOverrides(appearance: Appearance?, context: InputContext) {
        let changed = appearance != appearanceOverride || context != contextOverride
        appearanceOverride = appearance
        contextOverride = context
        guard changed else { return }
        for source in sources.values { source.onWake?(.appearanceChanged) }
    }

    // MARK: InputHubbing

    public var appearance: Appearance { appearanceOverride ?? base.appearance }

    public func source(for key: SiliconKey) -> any DrawingInputSource {
        if let source = sources[key] { return source }
        let source = SimulationInputSource(base: base.source(for: key), hub: self)
        sources[key] = source
        return source
    }

    public func update(_ key: SiliconKey, _ mutate: (inout BubbleInputState) -> Void) {
        base.update(key, mutate)
        if let state = base.state(for: key) { onStateChange?(key, state) }
    }

    public func state(for key: SiliconKey) -> BubbleInputState? { base.state(for: key) }

    public func remove(_ key: SiliconKey) {
        base.remove(key)
        sources.removeValue(forKey: key)
    }

    fileprivate func apply(to snapshot: inout InputSnapshot) {
        if let appearanceOverride { snapshot.appearance = appearanceOverride }
        snapshot.context = contextOverride
    }
}

/// Wraps the real per-bubble source and applies ``SimulationInputHub``'s overrides to each frame's input.
@MainActor
final class SimulationInputSource: DrawingInputSource {
    let base: any DrawingInputSource
    weak var hub: SimulationInputHub?

    init(base: any DrawingInputSource, hub: SimulationInputHub) {
        self.base = base
        self.hub = hub
    }

    /// Shared with the wrapped source, so its wake reasons reach the drawing host unchanged.
    var onWake: (@MainActor (WakeReason) -> Void)? {
        get { base.onWake }
        set { base.onWake = newValue }
    }

    func snapshot(t: Double, dt: Double) -> InputSnapshot {
        var snapshot = base.snapshot(t: t, dt: dt)
        hub?.apply(to: &snapshot)
        return snapshot
    }
}

/// The backdrop Simulation reports: a fixed light or dark tone from the toggle, or the app's live
/// sampler (desktop picture or screen) when the toggle says "live".
@MainActor
public final class SimulatedBackdrop: BackdropSampling {
    public enum Tone: Equatable, Sendable {
        case fixed(BackdropTone)
        case live
    }

    public private(set) var tone: Tone = .fixed(.light)
    public var onChange: (@MainActor (SiliconKey, Backdrop) -> Void)? {
        didSet { live.onChange = { [weak self] key, backdrop in self?.forwardLive(key, backdrop) } }
    }

    private let live: any BackdropSampling
    private var tracked: Set<SiliconKey> = []

    public init(live: any BackdropSampling) {
        self.live = live
    }

    public var source: BackdropSourceSetting {
        get { live.source }
        set { live.source = newValue }
    }

    public func setTone(_ newTone: Tone) {
        guard newTone != tone else { return }
        tone = newTone
        for key in tracked { onChange?(key, backdrop(for: key)) }
    }

    public func track(_ key: SiliconKey, rectOnScreen: CGRect?) {
        if rectOnScreen == nil { tracked.remove(key) } else { tracked.insert(key) }
        live.track(key, rectOnScreen: rectOnScreen)
    }

    public func backdrop(for key: SiliconKey) -> Backdrop {
        switch tone {
        case .fixed(let fixed): Self.fixedBackdrop(fixed)
        case .live: live.backdrop(for: key)
        }
    }

    public func warm(_ key: SiliconKey, rectOnScreen: CGRect?) {
        live.warm(key, rectOnScreen: rectOnScreen)
    }

    /// A fixed tone is always known (age 0); the live sampler reports its own.
    public func sampleAge(for key: SiliconKey) -> Double? {
        switch tone {
        case .fixed: 0
        case .live: live.sampleAge(for: key)
        }
    }

    /// The simulated samples: a pale warm desk for light, a deep blue-grey for dark.
    public static func fixedBackdrop(_ tone: BackdropTone) -> Backdrop {
        switch tone {
        case .light: Backdrop.sample(red: 0.93, green: 0.91, blue: 0.87, source: .wallpaper, previousTone: nil)
        case .dark: Backdrop.sample(red: 0.12, green: 0.14, blue: 0.19, source: .wallpaper, previousTone: nil)
        }
    }

    private func forwardLive(_ key: SiliconKey, _ backdrop: Backdrop) {
        guard tone == .live else { return }
        onChange?(key, backdrop)
    }
}
