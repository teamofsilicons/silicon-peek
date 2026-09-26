import AppKit
import CQuickJS
import PeekCore

/// Facts about the embedded QuickJS build.
public enum QuickJSInfo {
    /// The vendored quickjs-ng version, e.g. "0.17.0".
    public static var version: String { String(cString: peek_qjs_version()) }
}

/// Creates drawing hosts and validates scripts for peekd (visual.md B1, B11; BLUEPRINT §8.6, D15).
@MainActor
public final class DrawingRuntime: DrawingRuntimeProviding {
    /// `live` when the private `-[NSWindow _hasActiveAppearance]` override can give real Liquid Glass (D13).
    public let glassMode: GlassMode

    public init() {
        glassMode = NSWindow.instancesRespond(to: NSSelectorFromString("_hasActiveAppearance")) ? .live : .frosted
        Self.warmUpFonts()
    }

    /// For tests and Simulation: a runtime with a fixed glass mode.
    public init(glassMode: GlassMode) {
        self.glassMode = glassMode
        Self.warmUpFonts()
    }

    private static func warmUpFonts() {
        DispatchQueue.global(qos: .utility).async { FontCache.shared.warmUp() }
    }

    public func makeHost(for key: SiliconKey, initial: String, images: any ImageProviding) -> any DrawingHosting {
        DrawingHost(key: key, initial: initial, images: images, glassMode: glassMode)
    }

    /// `drawing.validate`: validates the file peekd staged at `url` in a temporary runtime (A9).
    public func validate(scriptAt url: URL, options: ValidationOptions) async -> ValidationReport {
        let data: Data
        do {
            let attributes = try FileManager.default.attributesOfItem(atPath: url.path)
            if let size = attributes[.size] as? Int, size > DrawingLimits.maxScriptBytes {
                return ValidationReport(ok: false, error: ValidationFailure(
                    message: String(format: "%@ is %.1f KB; drawings are limited to 256 KB (%@)",
                                    url.lastPathComponent, Double(size) / 1024, DrawingDocs.limits)))
            }
            data = try Data(contentsOf: url)
        } catch {
            return ValidationReport(ok: false, error: ValidationFailure(
                message: "cannot read the drawing at \(url.path): \(error.localizedDescription). "
                    + "peekd stages the script before asking Peek.app to validate it; register the drawing again"))
        }
        let script = DrawingScript(key: SiliconKey(context: .production, orgID: "", actorID: ""), sha256: "",
                                   source: data, filename: url.lastPathComponent)
        return await DrawingValidator.validate(script, options: options, glassMode: glassMode)
    }
}

/// A host that only shows the fallback visual (a glass circle with the Silicon's initial) and never runs a
/// script. PeekUI can use it for a Silicon whose drawing is unavailable.
@MainActor
public final class FallbackDrawingHost: DrawingHosting {
    public let key: SiliconKey
    public private(set) var status: DrawingHostStatus = .empty
    public var input: (any DrawingInputSource)?
    public var onFailure: (@MainActor (DrawingFailure) -> Void)?
    public var onLog: (@MainActor (String) -> Void)?

    private let initial: String
    private let glassMode: GlassMode
    private var view: FallbackVisualView?

    public init(key: SiliconKey, initial: String, glassMode: GlassMode = .live) {
        self.key = key
        self.initial = initial
        self.glassMode = glassMode
    }

    public func load(_ script: DrawingScript) async throws(DrawingFailure) {
        let failure = DrawingFailure(
            reason: .throwsRepeatedly,
            message: "this host shows only the fallback visual for \(key); \(script.filename) was not run")
        status = .fallback(failure)
        throw failure
    }

    public func unload() {
        status = .empty
    }

    public func attach(to visualView: NSView) {
        detach()
        let fallback = FallbackVisualView(initial: initial)
        fallback.frame = visualView.bounds
        visualView.addSubview(fallback)
        view = fallback
    }

    public func detach() {
        view?.removeFromSuperview()
        view = nil
    }

    public func deliver(_ event: DrawingEvent) {}

    public func wake() {}

    public func isOverContent(unitPoint: CGPoint) -> Bool { FallbackHitTest.contains(unitPoint) }

    public func validate(_ script: DrawingScript, options: ValidationOptions) async -> ValidationReport {
        await DrawingValidator.validate(script, options: options, glassMode: glassMode)
    }
}
