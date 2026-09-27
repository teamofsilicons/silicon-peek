// Records a Peek.app slide-in with ScreenCaptureKit and measures the colour of the visual while it lands
// (peek 0.1.2 contract §8.6 step 5). Built and run by scripts/capture-slide-in.sh; compiled together with PeekCore's
// sources so the panel and visual rectangles come from the same geometry code the app uses.
//
// It launches a Debug Peek.app in an isolated environment (PEEK_NO_SERVICES=1, temporary support/caches/socket, so
// nothing is registered with launchd and the real peekd is never contacted) with `--simulate <scenario>` over a
// vivid backdrop image, captures only Peek's own windows at 60 fps, reads the app's `PEEK_DEBUG_SLIDE_LOG` timestamps
// (CACurrentMediaTime, the same host clock as the frames' presentation times) and reports, for every frame from the
// slide-in to 600 ms after landing, the mean HSV saturation of the visual. A grey → colour snap after landing shows up
// as a saturation ramp. Pass: max frame-to-frame |Δsaturation| < 0.05 and total change < 0.08 after landing.

import AppKit
import CoreMedia
import CoreVideo
import Foundation
import ImageIO
import QuartzCore
import ScreenCaptureKit
import UniformTypeIdentifiers

// MARK: - Options

struct Options {
    var app = ""
    var scenario = "show-cover"
    var position = 5
    var mode = "normal"
    var fps = 60
    var scale: CGFloat = 1
    var out = ""
    var backdrop: String?
    var label = "capture"
    var hold = 20
    var timeout: Double = 15
    var frameStride = 2
    /// Who draws the backdrop: `tool` (this process: the glass samples another app's window, as in real use),
    /// `peek` (`--simulate-backdrop`, Peek's own Debug window) or `desktop` (whatever is on screen; only Peek's
    /// windows appear in the frames).
    var backdropOwner = "tool"
    /// `PEEK_DEBUG_PREWARM` for the app (transparent, offscreen, none), for A/B captures of one build.
    var prewarm: String?
    /// Present the scenario a second time after this many seconds (`--simulate-again`) and measure that slide-in: a
    /// later bubble on the reused panel.
    var again: Int?
    /// The scenario presented the second time (`--simulate-again <s>:<scenario>`).
    var againScenario: String?
    /// Stills to save, in ms after the measured slide settled (screenshots of the resting bubble).
    var stills: [Int] = []
    /// Extra environment for the app (`KEY=VALUE`, e.g. PEEK_DEBUG_HOVER=expand), Debug hooks only.
    var extraEnvironment: [String: String] = [:]

    static func parse(_ arguments: [String]) -> Options {
        var options = Options()
        var index = 1
        func value() -> String {
            index += 1
            guard index < arguments.count else { fail("\(arguments[index - 1]) needs a value") }
            return arguments[index]
        }
        while index < arguments.count {
            switch arguments[index] {
            case "--app": options.app = value()
            case "--scenario": options.scenario = value()
            case "--position": options.position = Int(value()) ?? 5
            case "--mode": options.mode = value()
            case "--fps": options.fps = Int(value()) ?? 60
            case "--scale": options.scale = CGFloat(Double(value()) ?? 1)
            case "--out": options.out = value()
            case "--backdrop": options.backdrop = value()
            case "--label": options.label = value()
            case "--hold": options.hold = Int(value()) ?? 20
            case "--timeout": options.timeout = Double(value()) ?? 15
            case "--frame-stride": options.frameStride = max(1, Int(value()) ?? 2)
            case "--prewarm": options.prewarm = value()
            case "--again": options.again = Int(value())
            case "--again-scenario": options.againScenario = value()
            case "--stills": options.stills = value().split(separator: ",").compactMap { Int($0) }
            case "--env":
                let pair = value().split(separator: "=", maxSplits: 1).map(String.init)
                guard pair.count == 2, pair[0].hasPrefix("PEEK_DEBUG_") else { fail("--env takes PEEK_DEBUG_*=value") }
                options.extraEnvironment[pair[0]] = pair[1]
            case "--backdrop-owner":
                options.backdropOwner = value()
                guard ["tool", "peek", "desktop"].contains(options.backdropOwner) else {
                    fail("--backdrop-owner must be tool, peek or desktop")
                }
            case "-h", "--help":
                print("usage: capture-slide-in --app <Peek.app> --out <dir> [--scenario show-cover] [--position 1-8] "
                    + "[--mode normal|compact] [--fps 60] [--scale 1] [--backdrop <png>] [--backdrop-owner tool|peek] "
                    + "[--prewarm transparent|offscreen|none] [--again <seconds>] [--again-scenario <name>] [--hold <seconds>] "
                    + "[--stills <ms,ms>] [--env PEEK_DEBUG_X=y] [--label before]")
                exit(0)
            default: fail("unknown argument \(arguments[index])")
            }
            index += 1
        }
        if options.app.isEmpty { fail("--app <path to a Debug Peek.app> is required") }
        if options.out.isEmpty { fail("--out <directory> is required") }
        return options
    }
}

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data("capture-slide-in: error: \(message)\n".utf8))
    exit(1)
}

func note(_ message: String) {
    FileHandle.standardError.write(Data("capture-slide-in: \(message)\n".utf8))
}

// MARK: - Frames

struct CapturedFrame {
    var time: Double
    var width: Int
    var height: Int
    var bytesPerRow: Int
    var pixels: Data  // BGRA, sRGB
}

final class FrameSink: NSObject, SCStreamOutput, SCStreamDelegate, @unchecked Sendable {
    private let lock = NSLock()
    private var frames: [CapturedFrame] = []
    private(set) var streamError: String?

    func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .screen, sampleBuffer.isValid,
              let attachments = CMSampleBufferGetSampleAttachmentsArray(sampleBuffer, createIfNecessary: false)
                as? [[SCStreamFrameInfo: Any]],
              let raw = attachments.first?[.status] as? Int, SCFrameStatus(rawValue: raw) == .complete,
              let buffer = sampleBuffer.imageBuffer else { return }
        let time = CMTimeGetSeconds(sampleBuffer.presentationTimeStamp)
        CVPixelBufferLockBaseAddress(buffer, .readOnly)
        defer { CVPixelBufferUnlockBaseAddress(buffer, .readOnly) }
        guard let base = CVPixelBufferGetBaseAddress(buffer) else { return }
        let height = CVPixelBufferGetHeight(buffer)
        let bytesPerRow = CVPixelBufferGetBytesPerRow(buffer)
        let frame = CapturedFrame(time: time, width: CVPixelBufferGetWidth(buffer), height: height, bytesPerRow: bytesPerRow,
                                  pixels: Data(bytes: base, count: bytesPerRow * height))
        lock.lock()
        frames.append(frame)
        lock.unlock()
    }

    func stream(_ stream: SCStream, didStopWithError error: any Error) {
        lock.lock()
        streamError = error.localizedDescription
        lock.unlock()
    }

    func take() -> [CapturedFrame] {
        lock.lock()
        defer { lock.unlock() }
        return frames
    }
}

extension CapturedFrame {
    /// Mean HSV saturation (0…1) and mean value over `rect` (pixels, top-left origin).
    func saturation(in rect: CGRect) -> (saturation: Double, value: Double) {
        let x0 = max(0, Int(rect.minX)), x1 = min(width, Int(rect.maxX))
        let y0 = max(0, Int(rect.minY)), y1 = min(height, Int(rect.maxY))
        guard x1 > x0, y1 > y0 else { return (0, 0) }
        var total = 0.0, totalValue = 0.0, count = 0.0
        pixels.withUnsafeBytes { raw in
            let p = raw.bindMemory(to: UInt8.self)
            for y in y0..<y1 {
                let row = y * bytesPerRow
                for x in x0..<x1 {
                    let i = row + x * 4
                    let b = Double(p[i]), g = Double(p[i + 1]), r = Double(p[i + 2])
                    let maximum = max(r, g, b), minimum = min(r, g, b)
                    total += maximum > 0 ? (maximum - minimum) / maximum : 0
                    totalValue += maximum / 255
                    count += 1
                }
            }
        }
        return (total / count, totalValue / count)
    }

    /// Mean absolute per-channel difference (0…255) from `other` over `rect`.
    func meanDifference(from other: CapturedFrame, in rect: CGRect) -> Double {
        guard other.width == width, other.height == height else { return 255 }
        let x0 = max(0, Int(rect.minX)), x1 = min(width, Int(rect.maxX))
        let y0 = max(0, Int(rect.minY)), y1 = min(height, Int(rect.maxY))
        guard x1 > x0, y1 > y0 else { return 0 }
        var total = 0.0, count = 0.0
        pixels.withUnsafeBytes { a in
            other.pixels.withUnsafeBytes { b in
                let p = a.bindMemory(to: UInt8.self), q = b.bindMemory(to: UInt8.self)
                for y in y0..<y1 {
                    for x in x0..<x1 {
                        let i = y * bytesPerRow + x * 4, j = y * other.bytesPerRow + x * 4
                        for c in 0..<3 { total += abs(Double(p[i + c]) - Double(q[j + c])) }
                        count += 3
                    }
                }
            }
        }
        return total / count
    }

    func cgImage() -> CGImage? {
        guard let provider = CGDataProvider(data: pixels as CFData) else { return nil }
        return CGImage(width: width, height: height, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: bytesPerRow,
                       space: CGColorSpace(name: CGColorSpace.sRGB)!,
                       bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedFirst.rawValue
                           | CGBitmapInfo.byteOrder32Little.rawValue),
                       provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent)
    }
}

func writePNG(_ image: CGImage, to url: URL) {
    guard let destination = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil) else {
        return
    }
    CGImageDestinationAddImage(destination, image, nil)
    CGImageDestinationFinalize(destination)
}

/// A vivid, saturated backdrop (magenta → orange → green), so live glass over it is clearly coloured and a frosted or
/// inactive (grey) rendering stands out.
func makeBackdrop(size: CGSize, at url: URL) {
    let width = Int(size.width), height = Int(size.height)
    guard let context = CGContext(data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
                                  space: CGColorSpace(name: CGColorSpace.sRGB)!,
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return }
    let colors = [CGColor(srgbRed: 0.85, green: 0.1, blue: 0.55, alpha: 1), CGColor(srgbRed: 0.98, green: 0.55, blue: 0.1, alpha: 1),
                  CGColor(srgbRed: 0.1, green: 0.7, blue: 0.35, alpha: 1)] as CFArray
    let gradient = CGGradient(colorsSpace: CGColorSpace(name: CGColorSpace.sRGB), colors: colors, locations: [0, 0.5, 1])!
    context.drawLinearGradient(gradient, start: .zero, end: CGPoint(x: width, y: height), options: [])
    if let image = context.makeImage() { writePNG(image, to: url) }
}

// MARK: - Main

@main
struct CaptureSlideIn {
    static func main() async {
        let options = Options.parse(CommandLine.arguments)
        let outDir = URL(fileURLWithPath: options.out, isDirectory: true)
        try? FileManager.default.createDirectory(at: outDir, withIntermediateDirectories: true)
        let executable = URL(fileURLWithPath: options.app).appendingPathComponent("Contents/MacOS/Peek")
        guard FileManager.default.isExecutableFile(atPath: executable.path) else { fail("\(executable.path) is not executable") }
        guard CGPreflightScreenCaptureAccess() else {
            fail("this process has no Screen Recording access (System Settings › Privacy & Security › Screen Recording)")
        }
        guard let screen = NSScreen.screens.first else { fail("no screen") }
        guard let slot = SlotIndex(rawValue: options.position), let mode = DisplayMode(rawValue: options.mode) else {
            fail("--position must be 1…8 and --mode normal|compact")
        }

        // Geometry, exactly as the app computes it (ScreenPolicy.visibleFrame(for: .main) = screens[0].visibleFrame).
        let layout = SlotGeometry.layout(slot: slot, mode: mode, visibleFrame: screen.visibleFrame)
        let primaryHeight = screen.frame.height
        let panel = layout.panelFrame
        let panelTop = CGRect(x: panel.minX, y: primaryHeight - panel.maxY, width: panel.width, height: panel.height)
        let visual = layout.visualFrameOnScreen
        let visualInPanel = CGRect(x: (visual.minX - panel.minX) * options.scale, y: (panel.maxY - visual.maxY) * options.scale,
                                   width: visual.width * options.scale, height: visual.height * options.scale)
        // The glass body: the middle of the visual square (the cassette shell fills it; the corners are backdrop).
        let glassRegion = visualInPanel.insetBy(dx: visualInPanel.width * 0.2, dy: visualInPanel.height * 0.25)
        note("panel \(Int(panelTop.minX)),\(Int(panelTop.minY)),\(Int(panelTop.width)),\(Int(panelTop.height)) (top-left points); "
            + "visual in panel \(visualInPanel.integral); measured region \(glassRegion.integral)")

        let backdropPath: String
        if let backdrop = options.backdrop {
            backdropPath = backdrop
        } else {
            let url = outDir.appendingPathComponent("backdrop.png")
            makeBackdrop(size: CGSize(width: screen.frame.width * screen.backingScaleFactor,
                                      height: screen.frame.height * screen.backingScaleFactor), at: url)
            backdropPath = url.path
        }

        // Isolated environment: nothing touches the real ~/Library/Application Support/Peek, launchd or peekd.
        let temp = FileManager.default.temporaryDirectory.appendingPathComponent("peek-capture-\(UUID().uuidString.prefix(8))")
        let support = temp.appendingPathComponent("support"), caches = temp.appendingPathComponent("caches")
        let sockDir = temp.appendingPathComponent("sock")
        for dir in [support, caches, sockDir] {
            try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        }
        defer { try? FileManager.default.removeItem(at: temp) }

        let process = Process()
        process.executableURL = executable
        process.arguments = ["--simulate", options.scenario, "--simulate-position", String(options.position),
                             "--simulate-hold", String(options.hold), "--simulate-mode", options.mode]
        if let again = options.again {
            process.arguments! += ["--simulate-again", options.againScenario.map { "\(again):\($0)" } ?? String(again)]
        }
        switch options.backdropOwner {
        case "peek": process.arguments! += ["--simulate-backdrop", backdropPath]
        case "tool": await showBackdropWindow(imagePath: backdropPath, on: screen)
        default: break  // "desktop": whatever is on screen (only Peek's own windows are captured)
        }
        var environment = ["HOME": NSHomeDirectory(), "PATH": "/usr/bin:/bin", "TMPDIR": NSTemporaryDirectory(),
                           "PEEK_NO_SERVICES": "1", "PEEK_SKIP_SERVICE_REGISTRATION": "1",
                           "PEEK_SUPPORT_DIR": support.path, "PEEK_CACHES_DIR": caches.path,
                           "PEEK_DAEMON_SOCKET": sockDir.appendingPathComponent("peekd.sock").path,
                           "PEEK_DEBUG_SLIDE_LOG": "1"]
        if let prewarm = options.prewarm { environment["PEEK_DEBUG_PREWARM"] = prewarm }
        environment.merge(options.extraEnvironment) { _, new in new }
        if let user = ProcessInfo.processInfo.environment["USER"] { environment["USER"] = user }
        process.environment = environment
        let stderrPipe = Pipe()
        process.standardError = stderrPipe
        process.standardOutput = FileHandle.nullDevice

        let events = SlideEvents()
        stderrPipe.fileHandleForReading.readabilityHandler = { handle in
            let data = handle.availableData
            guard !data.isEmpty, let text = String(data: data, encoding: .utf8) else { return }
            events.ingest(text)
        }

        func stopApp() {
            if process.isRunning {
                process.terminate()
                let deadline = Date().addingTimeInterval(3)
                while process.isRunning, Date() < deadline { usleep(50_000) }
                if process.isRunning { kill(process.processIdentifier, SIGKILL) }
            }
            stderrPipe.fileHandleForReading.readabilityHandler = nil
        }

        // Start capturing before the app exists: every application running now is excluded, so what is left is the
        // Peek launched next (its backdrop window covers everything else). Starting a stream takes a few hundred ms,
        // longer than the app needs to slide its first bubble in.
        guard let content = try? await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: false),
              let display = content.displays.first(where: { $0.displayID == CGMainDisplayID() }) ?? content.displays.first
        else { fail("ScreenCaptureKit lists no display") }
        let ownPID = ProcessInfo.processInfo.processIdentifier
        let filter = SCContentFilter(display: display,
                                     excludingApplications: content.applications.filter { $0.processID != ownPID },
                                     exceptingWindows: [])
        let configuration = SCStreamConfiguration()
        configuration.sourceRect = panelTop
        configuration.width = Int(panelTop.width * options.scale)
        configuration.height = Int(panelTop.height * options.scale)
        configuration.minimumFrameInterval = CMTime(value: 1, timescale: CMTimeScale(options.fps))
        configuration.pixelFormat = kCVPixelFormatType_32BGRA
        configuration.colorSpaceName = CGColorSpace.sRGB
        configuration.showsCursor = false
        configuration.queueDepth = 8
        let sink = FrameSink()
        let stream = SCStream(filter: filter, configuration: configuration, delegate: sink)
        do {
            try stream.addStreamOutput(sink, type: .screen, sampleHandlerQueue: DispatchQueue(label: "capture"))
            try await stream.startCapture()
        } catch {
            fail("could not start the capture: \(error.localizedDescription)")
        }

        do { try process.run() } catch { fail("could not launch \(executable.path): \(error)") }
        note("launched Peek (pid \(process.processIdentifier)) with --simulate \(options.scenario) at position \(options.position)")
        note("capturing Peek's windows at \(options.fps) fps (\(configuration.width)×\(configuration.height) px)")

        // Record until 1.2 s after the measured slide settled (or the timeout).
        let slides = options.again == nil ? 1 : 2
        let deadline = Date().addingTimeInterval(options.timeout + Double(options.again ?? 0))
        while Date() < deadline {
            let tail = max(1.2, Double(options.stills.max() ?? 0) / 1000 + 0.3)
            if events.times("settled").count >= slides, let settled = events.times("settled").last,
               CACurrentMediaTime() > settled + tail { break }
            try? await Task.sleep(for: .milliseconds(50))
        }
        try? await stream.stopCapture()
        stopApp()

        let frames = sink.take().sorted { $0.time < $1.time }
        guard events.times("in").count >= slides, let slideIn = events.times("in").last else {
            fail("the app logged \(events.times("in").count) slide-in(s), expected \(slides) (is it a Debug build with "
                + "PEEK_DEBUG_SLIDE_LOG support?); \(frames.count) frames captured; app said:\n\(events.log)")
        }
        let landed = events.times("landed").first { $0 >= slideIn } ?? (slideIn + 0.35)
        if let first = frames.first, let last = frames.last {
            note(String(format: "frames span %.3f … %.3f s (slide-in %.3f)", first.time, last.time, slideIn))
        }
        let prewarm = events.times("prewarm").last { $0 <= slideIn && $0 > slideIn - 2 }
        note(String(format: "slide-in at %.3f, landed %+.0f ms later%@; %d frames", slideIn, (landed - slideIn) * 1000,
                    prewarm.map { String(format: ", pre-warm began %.0f ms before the slide", (slideIn - $0) * 1000) } ?? "",
                    frames.count))

        // Per-frame colour of the visual, and the verdict.
        var csv = "t_ms_from_landing,t_ms_from_slide_in,saturation,value\n"
        var afterLanding: [(Double, Double)] = []
        let window = frames.filter { $0.time >= slideIn - 0.1 && $0.time <= landed + 0.6 }
        let framesDir = outDir.appendingPathComponent("frames")
        try? FileManager.default.createDirectory(at: framesDir, withIntermediateDirectories: true)
        var sheetImages: [(Double, CGImage)] = []
        for (index, frame) in window.enumerated() {
            let (saturation, value) = frame.saturation(in: glassRegion)
            let fromLanding = (frame.time - landed) * 1000, fromSlide = (frame.time - slideIn) * 1000
            csv += String(format: "%.1f,%.1f,%.4f,%.4f\n", fromLanding, fromSlide, saturation, value)
            if frame.time >= landed { afterLanding.append((fromLanding, saturation)) }
            if index % options.frameStride == 0, let image = frame.cgImage() {
                writePNG(image, to: framesDir.appendingPathComponent(String(format: "%@-%+05.0fms.png", options.label, fromLanding)))
                if frame.time >= landed - 0.15 { sheetImages.append((fromLanding, image)) }
            }
        }
        try? csv.write(to: outDir.appendingPathComponent("\(options.label)-saturation.csv"), atomically: true, encoding: .utf8)
        if let settled = events.times("settled").last {
            for ms in options.stills {
                let at = settled + Double(ms) / 1000
                guard let frame = frames.last(where: { $0.time <= at }), let image = frame.cgImage() else { continue }
                let url = outDir.appendingPathComponent("\(options.label)-\(ms)ms.png")
                writePNG(image, to: url)
                note("still: \(url.path)")
            }
        }
        writeContactSheet(sheetImages, region: glassRegion, to: outDir.appendingPathComponent("\(options.label)-contact-sheet.png"))

        var maxDelta = 0.0
        for index in afterLanding.indices.dropFirst() {
            maxDelta = max(maxDelta, abs(afterLanding[index].1 - afterLanding[index - 1].1))
        }
        let saturations = afterLanding.map(\.1)
        let total = (saturations.max() ?? 0) - (saturations.min() ?? 0)
        let passed = !afterLanding.isEmpty && maxDelta < 0.05 && total < 0.08
        let firstSlideFrame = frames.first { $0.time >= slideIn }.map { ($0.time - slideIn) * 1000 }
        var report = String(format: "%@: slide-in landed %.0f ms after it started%@; the first frame on screen after the slide "
                                + "started came %@\n", options.label, (landed - slideIn) * 1000,
                            prewarm.map { String(format: ", after a %.0f ms pre-warm", (slideIn - $0) * 1000) } ?? " (no pre-warm)",
                            firstSlideFrame.map { String(format: "%.0f ms later", $0) } ?? "never")
        report += String(format: "%@: %d frames from landing to +600 ms; saturation first %.3f, last %.3f, min %.3f, max %.3f; "
                                + "max frame-to-frame |Δ| %.3f (limit 0.05), total change %.3f (limit 0.08): %@\n",
                            options.label, afterLanding.count, saturations.first ?? 0, saturations.last ?? 0,
                            saturations.min() ?? 0, saturations.max() ?? 0, maxDelta, total, passed ? "PASS" : "FAIL")
        // The pre-warm must be invisible: from the pre-warm to the first slide frames, the visual's resting place shows
        // the backdrop only (no flash of the landed bubble when the window becomes fully opaque).
        if let prewarm, let baseline = frames.last(where: { $0.time < prewarm }) {
            let during = frames.filter { $0.time >= prewarm && $0.time <= slideIn + 0.05 }
            var worst = 0.0
            for frame in during { worst = max(worst, frame.meanDifference(from: baseline, in: visualInPanel)) }
            report += String(format: "pre-warm: %d frames from the pre-warm to 50 ms into the slide; the visual's resting place "
                                 + "differs from the backdrop by at most %.2f / 255 on average (%@)\n",
                             during.count, worst, worst < 3 ? "invisible" : "VISIBLE")
        }
        // The Esc router (Debug log): bare Esc held from the slide-in for the 3 s grace, then given back.
        if let held = events.times("esc-held").last(where: { $0 >= slideIn - 0.05 }) {
            let released = events.times("esc-released").first { $0 > held }
            report += String(format: "esc: held %.0f ms after the slide started, %@\n", (held - slideIn) * 1000,
                             released.map { String(format: "given back %.0f ms after it was taken", ($0 - held) * 1000) }
                                 ?? "not given back while recording")
        }
        report += "per frame (ms from landing: saturation):\n"
        for (t, s) in afterLanding { report += String(format: "  %+6.0f  %.3f\n", t, s) }
        try? report.write(to: outDir.appendingPathComponent("\(options.label)-report.txt"), atomically: true, encoding: .utf8)
        print(report, terminator: "")
        if let error = sink.streamError { note("stream error: \(error)") }
        exit(passed ? 0 : 2)
    }

    /// A grid of the captured frames from just before landing to +600 ms, each labelled, with the measured region boxed.
    static func writeContactSheet(_ images: [(Double, CGImage)], region: CGRect, to url: URL) {
        guard let first = images.first?.1 else { return }
        let picked = stride(from: 0, to: images.count, by: max(1, images.count / 12)).map { images[$0] }
        let columns = 4, rows = (picked.count + columns - 1) / columns
        let cellWidth = first.width / 2, cellHeight = first.height / 2 + 18
        guard let context = CGContext(data: nil, width: cellWidth * columns, height: cellHeight * rows, bitsPerComponent: 8,
                                      bytesPerRow: 0, space: CGColorSpace(name: CGColorSpace.sRGB)!,
                                      bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return }
        context.setFillColor(CGColor(gray: 0.08, alpha: 1))
        context.fill(CGRect(x: 0, y: 0, width: cellWidth * columns, height: cellHeight * rows))
        for (index, (t, image)) in picked.enumerated() {
            let column = index % columns, row = index / columns
            let origin = CGPoint(x: column * cellWidth, y: (rows - 1 - row) * cellHeight)
            let box = CGRect(x: origin.x, y: origin.y, width: CGFloat(cellWidth), height: CGFloat(cellHeight - 18))
            context.draw(image, in: box)
            context.setStrokeColor(CGColor(srgbRed: 1, green: 1, blue: 1, alpha: 0.9))
            context.setLineWidth(1)
            let scaleX = box.width / CGFloat(image.width), scaleY = box.height / CGFloat(image.height)
            context.stroke(CGRect(x: box.minX + region.minX * scaleX, y: box.maxY - region.maxY * scaleY,
                                  width: region.width * scaleX, height: region.height * scaleY))
            let label = NSAttributedString(string: String(format: "%+.0f ms", t), attributes: [
                .font: NSFont.monospacedDigitSystemFont(ofSize: 12, weight: .semibold), .foregroundColor: NSColor.white,
            ])
            NSGraphicsContext.saveGraphicsState()
            NSGraphicsContext.current = NSGraphicsContext(cgContext: context, flipped: false)
            label.draw(at: CGPoint(x: origin.x + 6, y: origin.y + CGFloat(cellHeight) - 15))
            NSGraphicsContext.restoreGraphicsState()
        }
        if let image = context.makeImage() { writePNG(image, to: url) }
    }
}

/// A click-through window of this process filling the screen with `imagePath`, just below Peek's panels (level
/// floating − 1) and above every other app, so the bubble's glass samples another process's window (as over a real
/// desktop) and nothing private from other apps is captured.
@MainActor
var backdropWindow: NSWindow?

@MainActor
func showBackdropWindow(imagePath: String, on screen: NSScreen) async {
    let app = NSApplication.shared
    app.setActivationPolicy(.accessory)
    app.finishLaunching()
    guard let image = NSImage(contentsOfFile: imagePath) else { fail("cannot read the backdrop image \(imagePath)") }
    let window = NSWindow(contentRect: screen.frame, styleMask: [.borderless], backing: .buffered, defer: false)
    window.isOpaque = true
    window.backgroundColor = .black
    window.hasShadow = false
    window.ignoresMouseEvents = true
    window.isReleasedWhenClosed = false
    window.animationBehavior = .none
    window.collectionBehavior = [.canJoinAllSpaces, .stationary, .ignoresCycle, .fullScreenAuxiliary]
    window.level = NSWindow.Level(rawValue: NSWindow.Level.floating.rawValue - 1)
    let view = NSView(frame: NSRect(origin: .zero, size: screen.frame.size))
    view.wantsLayer = true
    view.layer?.contentsGravity = .resizeAspectFill
    view.layer?.masksToBounds = true
    view.layer?.contents = image.cgImage(forProposedRect: nil, context: nil, hints: nil)
    window.contentView = view
    window.setFrame(screen.frame, display: true)
    window.orderFrontRegardless()
    CATransaction.flush()
    backdropWindow = window
    try? await Task.sleep(for: .milliseconds(300))
}

/// `peek slide: <what> t=<CACurrentMediaTime>` lines from the app's stderr.
final class SlideEvents: @unchecked Sendable {
    private let lock = NSLock()
    private var times: [String: [Double]] = [:]
    private var buffer = ""
    private(set) var log = ""

    func ingest(_ text: String) {
        lock.lock()
        defer { lock.unlock() }
        log += text
        buffer += text
        while let newline = buffer.firstIndex(of: "\n") {
            let line = String(buffer[..<newline])
            buffer.removeSubrange(...newline)
            guard line.hasPrefix("peek slide: ") else { continue }
            let parts = line.dropFirst("peek slide: ".count).split(separator: " ")
            guard parts.count == 2, parts[1].hasPrefix("t="), let t = Double(parts[1].dropFirst(2)) else { continue }
            times[String(parts[0]), default: []].append(t)
        }
    }

    /// Every time `name` was logged, in order.
    func times(_ name: String) -> [Double] {
        lock.lock()
        defer { lock.unlock() }
        return times[name] ?? []
    }
}
