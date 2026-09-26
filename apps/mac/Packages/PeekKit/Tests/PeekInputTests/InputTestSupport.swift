import AppKit
import Foundation
import ImageIO
import PeekCore
import Testing
import UniformTypeIdentifiers

@testable import PeekInput

// MARK: - Fakes for InputHub

@MainActor
final class FakeSpeech: SpeechPlaying {
    var onFinished: (@MainActor (SpeechFinished) -> Void)?
    var onFailed: (@MainActor (String, IPCErrorBody) -> Void)?
    var playbacks: [String: SpeechPlayback] = [:]
    var queried: [String] = []

    func handle(_ event: TTSStreamEvent) {}
    func stop(sendID: String) {}

    func playback(for sendID: String) -> SpeechPlayback? {
        queried.append(sendID)
        return playbacks[sendID]
    }

    func set(_ sendID: String, level: Double, progress: Double = 0.5, done: Bool = false) {
        playbacks[sendID] = SpeechPlayback(sendID: sendID, level: level, progress: progress, done: done, started: true,
                                           playedMs: 0, totalMs: nil)
    }
}

@MainActor
final class FakeMic: MicRecording {
    var permission: MicPermission = .granted
    var isRecording = false
    var level: Double = 0
    var onAutoStop: (@MainActor (MicRecordingResult) -> Void)?

    func requestPermission() async -> Bool { true }
    func start() throws(MicRecordingError) { isRecording = true }
    func stop() async throws(MicRecordingError) -> MicRecordingResult {
        isRecording = false
        return MicRecordingResult(wav: Data(), durationMs: 0, peakDBFS: -20)
    }
    func cancel() { isRecording = false }
}

/// A backdrop source that cannot notify InputHub (like Simulation's), so the hub has to poll it.
@MainActor
final class FakeBackdrop: BackdropSampling {
    var source: BackdropSourceSetting = .wallpaper
    var onChange: (@MainActor (SiliconKey, Backdrop) -> Void)?
    var values: [SiliconKey: Backdrop] = [:]
    var tracks: [(SiliconKey, CGRect?)] = []

    func track(_ key: SiliconKey, rectOnScreen: CGRect?) { tracks.append((key, rectOnScreen)) }
    func backdrop(for key: SiliconKey) -> Backdrop { values[key] ?? .fromAppearance(.light) }
}

@MainActor
final class FakePointer: PointerTracking {
    var location = CGPoint(x: -10_000, y: -10_000)
    var onMove: (@MainActor () -> Void)?
    private(set) var isMonitoring = false
    var starts = 0

    func startMonitoring() {
        isMonitoring = true
        starts += 1
    }

    func stopMonitoring() { isMonitoring = false }

    func move(to point: CGPoint) {
        location = point
        onMove?()
    }
}

@MainActor
final class FakeAppearance: AppearanceProviding {
    var current: Appearance = .light
    var onChange: (@MainActor (Appearance) -> Void)?

    func set(_ appearance: Appearance) {
        current = appearance
        onChange?(appearance)
    }
}

// MARK: - Fakes for BackdropSampler

@MainActor
final class FakeWallpaper: WallpaperProviding {
    var screens: [ScreenInfo]
    var keys: [UInt32: WallpaperKey] = [:]
    var snapshots: [WallpaperKey: WallpaperSnapshot] = [:]
    var decodes: [WallpaperKey] = []
    var onEnvironmentChange: (@MainActor () -> Void)?

    init(screens: [ScreenInfo]) { self.screens = screens }

    func currentKey(for screen: ScreenInfo) -> WallpaperKey? { keys[screen.id] }

    func snapshot(for key: WallpaperKey) async -> WallpaperSnapshot? {
        decodes.append(key)
        return snapshots[key]
    }

    /// Shows a flat colour on `screen`.
    func show(_ color: SRGBColor, on screen: ScreenInfo, name: String) {
        let key = WallpaperKey(url: URL(fileURLWithPath: "/wallpapers/\(name).png"), screen: screen)
        keys[screen.id] = key
        snapshots[key] = WallpaperSnapshot(bitmap: RGBABitmap(width: 16, height: 10, fill: color),
                                           imageFrame: CGRect(origin: .zero, size: screen.frame.size),
                                           fillColor: .black, screen: screen)
    }
}

@MainActor
final class FakeCapture: ScreenCapturing {
    var hasAccess = true
    var color = SRGBColor(red: 1, green: 1, blue: 1)
    var error: ScreenCaptureError?
    var captures: [CGRect] = []
    var invalidations = 0

    func invalidate() { invalidations += 1 }

    func averageColor(under rectOnScreen: CGRect) async throws(ScreenCaptureError) -> SRGBColor {
        captures.append(rectOnScreen)
        if let error { throw error }
        return color
    }
}

// MARK: - Images

enum TestImages {
    /// A PNG file whose pixels are drawn by `paint` (CoreGraphics coordinates, origin bottom-left).
    static func writePNG(width: Int, height: Int, to url: URL, paint: (CGContext) -> Void) throws {
        let context = try #require(CGContext(
            data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
            space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue))
        paint(context)
        let image = try #require(context.makeImage())
        let destination = try #require(CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil))
        CGImageDestinationAddImage(destination, image, nil)
        #expect(CGImageDestinationFinalize(destination))
    }

    static func image(width: Int, height: Int, paint: (CGContext) -> Void) throws -> CGImage {
        let context = try #require(CGContext(
            data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
            space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue))
        paint(context)
        return try #require(context.makeImage())
    }

    static func fill(_ context: CGContext, _ color: SRGBColor, _ rect: CGRect) {
        context.setFillColor(CGColor(srgbRed: color.red, green: color.green, blue: color.blue, alpha: 1))
        context.fill(rect)
    }
}

/// A temporary directory removed at the end of the test.
struct TemporaryDirectory: ~Copyable {
    let url: URL

    init() throws {
        url = FileManager.default.temporaryDirectory.appendingPathComponent("peek-input-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    }

    deinit { try? FileManager.default.removeItem(at: url) }
}

func close(_ a: Double, _ b: Double, _ tolerance: Double = 1e-9) -> Bool { abs(a - b) <= tolerance }

func hexDistance(_ a: String, _ b: String) -> Int {
    func components(_ hex: String) -> [Int] {
        let digits = Array(hex.dropFirst())
        return stride(from: 0, to: 6, by: 2).map { Int(String(digits[$0..<$0 + 2]), radix: 16) ?? 0 }
    }
    return zip(components(a), components(b)).map { abs($0 - $1) }.max() ?? 255
}
