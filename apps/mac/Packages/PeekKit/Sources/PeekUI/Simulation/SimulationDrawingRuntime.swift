import AppKit
import PeekCore

/// The live drawing runtime, seen through Simulation: hosts are the real ones (same QuickJS, same
/// compositor), wrapped so the Simulation window receives every `peek.log` line and fallback
/// report whatever the presenter does with its own `onLog` / `onFailure` hooks.
@MainActor
public final class SimulationDrawingRuntime: DrawingRuntimeProviding {
    public let base: any DrawingRuntimeProviding
    /// `peek.log` output of a simulated drawing.
    public var onLog: (@MainActor (SiliconKey, String) -> Void)?
    /// A simulated drawing switched to the fallback visual.
    public var onFailure: (@MainActor (SiliconKey, DrawingFailure) -> Void)?

    /// Every wrapper still alive, for reuse when the runtime hands out the same inner host again.
    private var hosts: [SiliconKey: [WeakHost]] = [:]
    /// The newest wrapper per key, kept even if the presenter let go of it, so the window can show its status.
    private var latest: [SiliconKey: SimulationDrawingHost] = [:]

    public init(base: any DrawingRuntimeProviding) {
        self.base = base
    }

    public var glassMode: GlassMode { base.glassMode }

    public func makeHost(for key: SiliconKey, initial: String, images: any ImageProviding) -> any DrawingHosting {
        let inner = base.makeHost(for: key, initial: initial, images: images)
        hosts[key, default: []].removeAll { $0.host == nil }
        if let existing = hosts[key]?.lazy.compactMap(\.host).first(where: { $0.inner === inner }) {
            latest[key] = existing
            return existing
        }
        let host = SimulationDrawingHost(inner: inner, runtime: self)
        hosts[key, default: []].append(WeakHost(host: host))
        latest[key] = host
        return host
    }

    public func validate(scriptAt url: URL, options: ValidationOptions) async -> ValidationReport {
        await base.validate(scriptAt: url, options: options)
    }

    /// The host most recently handed out for `key`.
    public func host(for key: SiliconKey) -> (any DrawingHosting)? { latest[key] }

    /// Destroys the VMs of every simulated host for `key` and forgets them.
    public func unloadAll(for key: SiliconKey) {
        var all = hosts.removeValue(forKey: key)?.compactMap(\.host) ?? []
        if let newest = latest.removeValue(forKey: key), !all.contains(where: { $0 === newest }) { all.append(newest) }
        for host in all { host.unload() }
    }

    fileprivate func log(_ key: SiliconKey, _ line: String) { onLog?(key, line) }
    fileprivate func failed(_ key: SiliconKey, _ failure: DrawingFailure) { onFailure?(key, failure) }

    private struct WeakHost {
        weak var host: SimulationDrawingHost?
    }
}

/// Forwards everything to the real host and taps its `onLog` / `onFailure`.
@MainActor
final class SimulationDrawingHost: DrawingHosting {
    let inner: any DrawingHosting
    private weak var runtime: SimulationDrawingRuntime?

    var onFailure: (@MainActor (DrawingFailure) -> Void)?
    var onLog: (@MainActor (String) -> Void)?

    init(inner: any DrawingHosting, runtime: SimulationDrawingRuntime) {
        self.inner = inner
        self.runtime = runtime
        let key = inner.key
        inner.onLog = { [weak self, weak runtime] line in
            runtime?.log(key, line)
            self?.onLog?(line)
        }
        inner.onFailure = { [weak self, weak runtime] failure in
            runtime?.failed(key, failure)
            self?.onFailure?(failure)
        }
    }

    var key: SiliconKey { inner.key }
    var status: DrawingHostStatus { inner.status }

    var input: (any DrawingInputSource)? {
        get { inner.input }
        set { inner.input = newValue }
    }

    func load(_ script: DrawingScript) async throws(DrawingFailure) { try await inner.load(script) }
    func unload() { inner.unload() }
    func attach(to visualView: NSView) { inner.attach(to: visualView) }
    func detach() { inner.detach() }
    func deliver(_ event: DrawingEvent) { inner.deliver(event) }
    func wake() { inner.wake() }
    func isOverContent(unitPoint: CGPoint) -> Bool { inner.isOverContent(unitPoint: unitPoint) }

    func validate(_ script: DrawingScript, options: ValidationOptions) async -> ValidationReport {
        await inner.validate(script, options: options)
    }

    func awaitFrame(timeout: Duration) async -> Bool { await inner.awaitFrame(timeout: timeout) }
}
