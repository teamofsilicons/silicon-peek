import AppKit
import Foundation
import OSLog
import PeekCore

/// Estimates what each visible bubble sits on (`input.backdrop`, pill shading; BLUEPRINT §8.5, §8.8, visual.md B8):
///
/// 1. `screen` (opt-in in Settings): a 16 × 16 ScreenCaptureKit capture under the bubble, excluding Peek's own
///    windows, at 2 Hz while tracked. Needs Screen Recording; without it the wallpaper is used.
/// 2. `wallpaper` (default): the desktop picture under the bubble, from a ≤ 256 px thumbnail cached per screen and
///    re-checked on space changes, screen changes and every 60 s.
/// 3. `appearance`: the last resort (dark mode = dark backdrop).
///
/// Luminance is Rec. 709 on linearised sRGB; the tone flips with hysteresis at 0.45 / 0.55 and `ink` is the
/// opposite of the tone (``Backdrop/sample(red:green:blue:source:previousTone:)``).
///
/// Two cadences (peek 0.1.2): keys **tracked** (a send arrived or its bubble is on screen) are sampled at once and then
/// at 2 Hz with the `screen` source; keys **warmed** (an occupied slot with nothing on screen) are sampled once and then
/// kept fresh at an idle cadence (every wallpaper refresh; every 10 s with the `screen` source), so a bubble's pill
/// shade is already known when it arrives and never changes after it lands.
@MainActor
public final class BackdropSampler: BackdropSampling {
    public static let wallpaperRefreshInterval: Duration = .seconds(60)
    public static let screenSampleInterval: Duration = .milliseconds(500)
    public static let idleScreenSampleInterval: Duration = .seconds(10)

    public var source: BackdropSourceSetting {
        didSet {
            guard source != oldValue else { return }
            updateTimers()
            resampleAll()
        }
    }

    public var onChange: (@MainActor (SiliconKey, Backdrop) -> Void)?

    /// Peek's paths; the samples live in memory (a thumbnail per screen), so nothing is written under them.
    public let paths: PeekPaths

    /// How often the desktop picture is re-checked while a bubble is tracked (default 60 s).
    public let refreshInterval: Duration
    /// How often the screen is sampled with the `screen` source (default 2 Hz).
    public let sampleInterval: Duration
    /// How often warmed (idle) keys are sampled with the `screen` source (default 10 s).
    public let idleSampleInterval: Duration
    /// Monotonic seconds (sample ages); injectable for tests.
    public var clock: @MainActor () -> Double = { Double(DispatchTime.now().uptimeNanoseconds) / 1e9 }

    private let wallpaper: any WallpaperProviding
    private let capture: any ScreenCapturing
    private let appearance: any AppearanceProviding
    private var tracked: [SiliconKey: CGRect] = [:]
    private var warmed: [SiliconKey: CGRect] = [:]
    private var current: [SiliconKey: Backdrop] = [:]
    private var sampledAt: [SiliconKey: Double] = [:]
    private var wallpapers: [UInt32: CachedWallpaper] = [:]
    private var captureInFlight: Set<SiliconKey> = []
    private var observers: [Int: @MainActor (SiliconKey, Backdrop) -> Void] = [:]
    private var nextObserverID = 0
    private var wallpaperTimer: Task<Void, Never>?
    private var screenTimer: Task<Void, Never>?
    private var idleScreenTimer: Task<Void, Never>?
    private let logger = PeekLogger(category: "backdrop")

    private struct CachedWallpaper {
        var key: WallpaperKey?
        var snapshot: WallpaperSnapshot?
        var loading: Task<Void, Never>?
    }

    public init(paths: PeekPaths, source: BackdropSourceSetting = .wallpaper,
                wallpaper: any WallpaperProviding = SystemWallpaper(),
                capture: any ScreenCapturing = ScreenCaptureKitSampler(),
                appearance: any AppearanceProviding = SystemAppearance(),
                refreshInterval: Duration = BackdropSampler.wallpaperRefreshInterval,
                sampleInterval: Duration = BackdropSampler.screenSampleInterval,
                idleSampleInterval: Duration = BackdropSampler.idleScreenSampleInterval) {
        self.paths = paths
        self.refreshInterval = refreshInterval
        self.sampleInterval = sampleInterval
        self.idleSampleInterval = idleSampleInterval
        self.source = source
        self.wallpaper = wallpaper
        self.capture = capture
        self.appearance = appearance
        wallpaper.onEnvironmentChange = { [weak self] in self?.refreshWallpapers() }
        appearance.onChange = { [weak self] _ in self?.appearanceChanged() }
    }

    isolated deinit {
        wallpaperTimer?.cancel()
        screenTimer?.cancel()
        idleScreenTimer?.cancel()
    }

    // MARK: BackdropSampling

    public func track(_ key: SiliconKey, rectOnScreen: CGRect?) {
        guard let rect = rectOnScreen, rect.width > 0, rect.height > 0 else {
            tracked.removeValue(forKey: key)
            updateTimers()
            return
        }
        guard tracked[key] != rect else { return }
        tracked[key] = rect
        updateTimers()
        resample(key)
    }

    public func warm(_ key: SiliconKey, rectOnScreen: CGRect?) {
        guard let rect = rectOnScreen, rect.width > 0, rect.height > 0 else {
            warmed.removeValue(forKey: key)
            updateTimers()
            return
        }
        guard warmed[key] != rect else { return }
        warmed[key] = rect
        updateTimers()
        // A tracked key is sampled at its own (faster) cadence; a new idle key gets its first sample now.
        if tracked[key] == nil { resample(key) }
    }

    public func sampleAge(for key: SiliconKey) -> Double? {
        sampledAt[key].map { max(0, clock() - $0) }
    }

    public func backdrop(for key: SiliconKey) -> Backdrop {
        current[key] ?? .fromAppearance(appearance.current)
    }

    /// Adds a change listener next to ``onChange`` (InputHub uses this to wake drawings while PeekUI owns
    /// `onChange`). Returns a token for ``removeObserver(_:)``.
    @discardableResult
    public func addObserver(_ observer: @escaping @MainActor (SiliconKey, Backdrop) -> Void) -> Int {
        nextObserverID += 1
        observers[nextObserverID] = observer
        return nextObserverID
    }

    public func removeObserver(_ token: Int) {
        observers.removeValue(forKey: token)
    }

    /// Keys being sampled now.
    public var trackedKeys: Set<SiliconKey> { Set(tracked.keys) }

    /// Keys kept warm at the idle cadence.
    public var warmedKeys: Set<SiliconKey> { Set(warmed.keys) }

    /// Screen captures started and not finished yet (diagnostics and tests).
    public var pendingCaptureCount: Int { captureInFlight.count }

    /// The source actually in use: `screen` only with Screen Recording access.
    public var effectiveSource: BackdropSource {
        source == .screen && capture.hasAccess ? .screen : .wallpaper
    }

    // MARK: Sampling

    /// Samples every tracked and warmed key now.
    public func resampleAll() {
        let keys = Set(tracked.keys).union(warmed.keys)
        for key in keys.sorted(by: { $0.description < $1.description }) { resample(key) }
    }

    private func resampleTracked() {
        for key in tracked.keys.sorted(by: { $0.description < $1.description }) { resample(key) }
    }

    private func resampleWarmedIdle() {
        for key in warmed.keys.sorted(by: { $0.description < $1.description }) where tracked[key] == nil { resample(key) }
    }

    /// Where `key` is sampled: its tracked rectangle, else its warmed one.
    private func rect(for key: SiliconKey) -> CGRect? { tracked[key] ?? warmed[key] }

    private func resample(_ key: SiliconKey) {
        guard let rect = rect(for: key) else { return }
        if effectiveSource == .screen {
            sampleScreen(key, rect: rect)
        } else {
            sampleWallpaper(key, rect: rect)
        }
    }

    private func sampleScreen(_ key: SiliconKey, rect: CGRect) {
        guard captureInFlight.insert(key).inserted else { return }
        let capture = self.capture
        Task { [weak self] in
            let result: Result<SRGBColor, ScreenCaptureError>
            do throws(ScreenCaptureError) {
                result = .success(try await capture.averageColor(under: rect))
            } catch {
                result = .failure(error)
            }
            guard let self else { return }
            self.captureInFlight.remove(key)
            guard self.rect(for: key) == rect else { return }
            switch result {
            case .success(let color):
                self.apply(color, source: .screen, to: key)
            case .failure(let error):
                self.logger.notice("screen backdrop failed, using the wallpaper: \(error.description)")
                self.sampleWallpaper(key, rect: rect)
            }
        }
    }

    private func sampleWallpaper(_ key: SiliconKey, rect: CGRect) {
        guard let screen = ScreenInfo.screen(for: rect, in: wallpaper.screens) else {
            applyAppearance(to: key)
            return
        }
        let cached = ensureWallpaper(for: screen)
        guard let snapshot = cached?.snapshot else {
            // Still decoding: keep the last value; the load resamples when done. Nothing readable: appearance.
            if cached?.loading == nil { applyAppearance(to: key) }
            return
        }
        guard let color = snapshot.averageColor(under: rect) else {
            applyAppearance(to: key)
            return
        }
        apply(color, source: .wallpaper, to: key)
    }

    /// The cached wallpaper for `screen`, starting a decode when the key changed.
    private func ensureWallpaper(for screen: ScreenInfo) -> CachedWallpaper? {
        guard let key = wallpaper.currentKey(for: screen) else {
            wallpapers[screen.id] = CachedWallpaper(key: nil, snapshot: nil, loading: nil)
            return nil
        }
        if let cached = wallpapers[screen.id], cached.key == key { return cached }
        wallpapers[screen.id]?.loading?.cancel()
        let provider = wallpaper
        let loading = Task { [weak self] in
            let snapshot = await provider.snapshot(for: key)
            guard let self, !Task.isCancelled, self.wallpapers[screen.id]?.key == key else { return }
            self.wallpapers[screen.id] = CachedWallpaper(key: key, snapshot: snapshot, loading: nil)
            let keys = Set(self.tracked.keys).union(self.warmed.keys)
            for sampledKey in keys.sorted(by: { $0.description < $1.description }) {
                guard let rect = self.rect(for: sampledKey), ScreenInfo.screen(for: rect, in: [screen]) != nil else { continue }
                self.sampleWallpaper(sampledKey, rect: rect)
            }
        }
        let entry = CachedWallpaper(key: key, snapshot: nil, loading: loading)
        wallpapers[screen.id] = entry
        return entry
    }

    /// Re-reads each screen's desktop picture key (space or screen change, 60 s timer), forgets cached displays,
    /// and resamples.
    public func refreshWallpapers() {
        capture.invalidate()
        let screens = Set(wallpaper.screens.map(\.id))
        for id in wallpapers.keys where !screens.contains(id) { wallpapers.removeValue(forKey: id) }
        resampleAll()
    }

    private func apply(_ color: SRGBColor, source: BackdropSource, to key: SiliconKey) {
        let next = Backdrop.sample(red: color.red, green: color.green, blue: color.blue, source: source,
                                   previousTone: current[key]?.tone)
        publish(next, for: key)
    }

    private func applyAppearance(to key: SiliconKey) {
        publish(.fromAppearance(appearance.current), for: key)
    }

    private func publish(_ next: Backdrop, for key: SiliconKey) {
        let previous = current[key]
        current[key] = next
        sampledAt[key] = clock()
        guard previous.map({ $0.tone != next.tone || $0.color != next.color || $0.source != next.source }) ?? true
        else { return }
        onChange?(key, next)
        for token in observers.keys.sorted() { observers[token]?(key, next) }
    }

    private func appearanceChanged() {
        for (key, backdrop) in current where backdrop.source == .appearance { applyAppearance(to: key) }
    }

    // MARK: Timers

    private func updateTimers() {
        let active = !tracked.isEmpty || !warmed.isEmpty
        if active, wallpaperTimer == nil {
            wallpaperTimer = repeating(every: refreshInterval) { $0.refreshWallpapers() }
        } else if !active {
            wallpaperTimer?.cancel()
            wallpaperTimer = nil
        }
        let screenActive = !tracked.isEmpty && source == .screen
        if screenActive, screenTimer == nil {
            screenTimer = repeating(every: sampleInterval) { sampler in
                if sampler.effectiveSource == .screen { sampler.resampleTracked() }
            }
        } else if !screenActive {
            screenTimer?.cancel()
            screenTimer = nil
        }
        let idleActive = !warmed.isEmpty && source == .screen
        if idleActive, idleScreenTimer == nil {
            idleScreenTimer = repeating(every: idleSampleInterval) { sampler in
                if sampler.effectiveSource == .screen { sampler.resampleWarmedIdle() }
            }
        } else if !idleActive {
            idleScreenTimer?.cancel()
            idleScreenTimer = nil
        }
    }

    private func repeating(every interval: Duration, _ body: @escaping @MainActor (BackdropSampler) -> Void)
        -> Task<Void, Never>
    {
        Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: interval)
                guard !Task.isCancelled, let self else { return }
                body(self)
            }
        }
    }
}
