import CoreGraphics
import CoreText
import Foundation
import ImageIO
import PeekCore
import UniformTypeIdentifiers

/// visual.md A9: runs a drawing offscreen for 90 frames in a separate, temporary runtime on its own 4 MiB
/// thread, with the same prelude, decoder and renderer as the live host (D15), and reports the
/// `drawing.validate` result (§1.6).
///
/// Fails when the script throws (at load, in `frame()` or in a handler), when a frame is interrupted on
/// 5 test frames or the p95 frame time is over 4 ms, when it hits the 16 MB memory limit or the 256 KB
/// size limit, or when it draws nothing at all. Warns (but accepts) when a glass outline changes in more
/// than 10% of the frames, when text is drawn in compact mode, when ops are dropped at the 5,000 cap,
/// when glass fills beyond 3 are drawn flat, and when values are ignored (unknown colours, fonts, …).
public enum DrawingValidator {
    public static func validate(_ script: DrawingScript, options: ValidationOptions = ValidationOptions(),
                                glassMode: GlassMode = .live) async -> ValidationReport {
        if script.source.count > DrawingLimits.maxScriptBytes {
            let kb = Double(script.source.count) / 1024
            return ValidationReport(ok: false, error: ValidationFailure(
                message: String(format: "%@ is %.1f KB; drawings are limited to 256 KB (%@). "
                    + "Remove unused code or data and register it again.", script.filename, kb, DrawingDocs.limits)))
        }
        guard let source = String(validating: script.source, as: UTF8.self) else {
            return ValidationReport(ok: false, error: ValidationFailure(
                message: "\(script.filename) is not valid UTF-8 text; save it as UTF-8 and register it again"))
        }
        let thread = JSThread(name: "ai.tos.peek.drawing.validate")
        defer { thread.finish() }
        let filename = script.filename
        return await thread.run {
            ValidationRun(source: source, filename: filename, options: options, glassMode: glassMode).execute()
        }
    }
}

/// One validation, entirely on the validator's thread.
private struct ValidationRun {
    let source: String
    let filename: String
    let options: ValidationOptions
    let glassMode: GlassMode

    private final class LogSink {
        var lines: [String] = []
        /// Prelude warnings (`warning: …`, once per kind): values peek ignored, reported as `ignored_value`.
        var warnings: [String] = []
        let limit = 200
        static let warningPrefix = "warning: "
        func append(_ line: String) {
            if line.hasPrefix(Self.warningPrefix) {
                let message = String(line.dropFirst(Self.warningPrefix.count))
                if !warnings.contains(message) { warnings.append(message) }
                return
            }
            if lines.count < limit {
                lines.append(line)
            } else if lines.count == limit {
                lines.append("… (more log lines were dropped)")
            }
        }
    }

    // swiftlint:disable:next function_body_length cyclomatic_complexity
    func execute() -> ValidationReport {
        // Font loading is slow the first time; keep it out of the measured frames.
        FontCache.shared.warmUp()
        let sink = LogSink()
        let engine: JSEngine
        do {
            engine = try JSEngine(log: { sink.append($0) })
        } catch {
            return ValidationReport(ok: false, error: ValidationFailure(message: "\(error)"))
        }

        let load = engine.evaluate(Array(source.utf8), filename: filename, budget: DrawingLimits.loadBudget)
        switch load {
        case .ok:
            break
        case .threw(let message, let stack):
            return failure("loading \(filename) threw: \(message)", stack: clean(stack), logs: sink.lines)
        case .interrupted:
            return failure(
                "the top-level code of \(filename) ran longer than \(millis(DrawingLimits.loadBudget)) ms; it runs "
                    + "once at load and must finish quickly (move work into peek.frame or spread it over frames)",
                logs: sink.lines)
        case .outOfMemory:
            return failure("loading \(filename) exceeded the 16 MB memory limit (\(DrawingDocs.limits))",
                           logs: sink.lines)
        case .failed(let message):
            return failure("loading \(filename) failed: \(message)", logs: sink.lines)
        }

        let images = ValidationSchedule.sampleImages()
        let steps = ValidationSchedule.steps(glass: glassMode)
        let decoder = OpDecoder()
        let previewFrames = Set((0..<12).map { $0 * ValidationSchedule.frameCount / 12 + 3 })
        var previewCells: [(Int, ValidationStep, DisplayList)] = []

        var times: [Double] = []
        var okFrames = 0
        var interrupted = 0
        var opsMax = 0
        var maxDropped = 0
        var glassOverflowFrames = 0
        var glassRebuilds = 0
        var outlineChanges: [Int: Int] = [:]
        var previousOutline: [Int: Hash64] = [:]
        var previousGlassHash: [Int: Hash64] = [:]
        var textInCompactFrame: Int?
        var drewSomething = false
        var warnings: [ValidationWarning] = []
        var dump: JSONValue?
        var checkedFrameFunction = false

        for step in steps {
            for event in step.events {
                let status = engine.event(name: event.name, payload: event.payloadJSON, budget: DrawingLimits.eventBudget)
                switch status {
                case .ok:
                    continue
                // `frame` is set only for exceptions: the CLI prints "frame N threw: <message>" from it, so the message
                // never repeats that prefix. Other failures name their frame in the message and leave `frame` unset.
                case .threw(let message, let stack):
                    return failure("\(message) (in the '\(event.name)' handler)", stack: clean(stack),
                                   frame: step.index, summary: step.summary, logs: sink.lines)
                case .interrupted:
                    return failure("the '\(event.name)' handler ran longer than 4 ms at frame \(step.index) "
                                   + "(\(DrawingDocs.limits))", summary: step.summary, logs: sink.lines)
                case .outOfMemory:
                    return failure("the '\(event.name)' handler exceeded the 16 MB memory limit at frame \(step.index) "
                                   + "(\(DrawingDocs.limits))", summary: step.summary, logs: sink.lines)
                case .failed(let message):
                    return failure("delivering '\(event.name)' at frame \(step.index) failed: \(message)",
                                   summary: step.summary, logs: sink.lines)
                }
            }

            let frame = engine.frame(input: step.input.jsonBytes(), budget: DrawingLimits.frameBudget)
            // Statistics use the VM thread's CPU time: what the drawing costs, not how busy the Mac is.
            // The 4 ms interrupt deadline itself is wall-clock, exactly as at runtime.
            let ms = Double(frame.cpuNanoseconds) / 1_000_000
            switch frame.status {
            case .ok:
                break
            case .threw(let message, let stack):
                return failure(message, stack: clean(stack), frame: step.index,
                               summary: step.summary, logs: sink.lines,
                               stats: stats(okFrames, times, opsMax, glassRebuilds))
            case .interrupted:
                times.append(max(ms, 4))
                interrupted += 1
                if interrupted >= 5 {
                    return failure(
                        "frame \(step.index) was interrupted: frame() ran longer than 4 ms on \(interrupted) test "
                            + "frames (\(DrawingDocs.limits)). Do less work per frame, precompute at the top level, "
                            + "and return false when nothing moves",
                        summary: step.summary, logs: sink.lines,
                        stats: stats(okFrames, times, opsMax, glassRebuilds))
                }
                continue
            case .outOfMemory:
                return failure("frame \(step.index) exceeded the 16 MB memory limit (\(DrawingDocs.limits))",
                               summary: step.summary, logs: sink.lines, stats: stats(okFrames, times, opsMax, glassRebuilds))
            case .failed(let message):
                return failure("frame \(step.index) failed: \(message)", summary: step.summary, logs: sink.lines)
            }
            times.append(ms)
            okFrames += 1

            let list = decoder.decode(ops: frame.ops, strings: frame.strings, again: frame.again, images: images,
                                      recordDump: options.dumpFrame == step.index)
            if !checkedFrameFunction {
                checkedFrameFunction = true
                if !list.hasFrameFunction {
                    return failure("\(filename) never called peek.frame(fn), so it cannot draw anything "
                                   + "(\(DrawingDocs.scriptStructure))", logs: sink.lines)
                }
            }
            opsMax = max(opsMax, list.recordedOps + list.droppedOps)
            maxDropped = max(maxDropped, list.droppedOps)
            if list.glassOverflow > 0 { glassOverflowFrames += 1 }
            // Glass overflow is reported once, as `glass_limit`, below.
            for message in list.diagnostics where message != OpDecoder.glassOverflowMessage
                && !warnings.contains(where: { $0.code == "ignored_value" && $0.message == message }) {
                warnings.append(ValidationWarning(code: "ignored_value", message: message))
            }
            if step.input.mode == .compact, list.textCount > 0, textInCompactFrame == nil {
                textInCompactFrame = step.index
            }

            var glassIndex = 0
            for layer in list.layers {
                switch layer {
                case .glass(let glass):
                    glassIndex += 1
                    if let previous = previousOutline[glassIndex], previous != glass.outlineHash {
                        outlineChanges[glassIndex, default: 0] += 1
                    }
                    if previousGlassHash[glassIndex] != glass.hash { glassRebuilds += 1 }
                    previousOutline[glassIndex] = glass.outlineHash
                    previousGlassHash[glassIndex] = glass.hash
                    drewSomething = true
                case .blur(let blur):
                    glassIndex += 1
                    if previousGlassHash[glassIndex] != blur.hash { glassRebuilds += 1 }
                    previousGlassHash[glassIndex] = blur.hash
                    drewSomething = true
                case .draw(let draw):
                    if !drewSomething, list.paintCount > 0,
                       let image = CGReplay.render(draw.commands, pixels: 64, environment: ReplayEnvironment(images: images),
                                                   colorSpace: CGReplay.sRGB),
                       Self.hasVisiblePixels(image) {
                        drewSomething = true
                    }
                }
            }
            if options.dumpFrame == step.index {
                dump = Self.dump(list, frame: step.index, summary: step.summary)
            }
            if options.preview, previewFrames.contains(step.index) {
                previewCells.append((step.index, step, list))
            }
        }

        if let frame = options.dumpFrame, dump == nil {
            warnings.append(ValidationWarning(
                code: "dump_frame_unavailable",
                message: "frame \(frame) could not be dumped: frames are numbered 0…\(ValidationSchedule.frameCount - 1) "
                    + "and interrupted frames have no display list"))
        }

        for message in sink.warnings
        where !warnings.contains(where: { $0.code == "ignored_value" && $0.message == message }) {
            warnings.append(ValidationWarning(code: "ignored_value", message: message))
        }

        let finalStats = stats(okFrames, times, opsMax, glassRebuilds)
        if finalStats.p95Ms > 4 {
            return failure(
                String(format: "frame time p95 %.2f ms is over the 4 ms limit (p50 %.2f ms, max %.2f ms; see %@). "
                    + "Do less work per frame and return false when nothing moves",
                    finalStats.p95Ms, finalStats.p50Ms, finalStats.maxMs, DrawingDocs.limits),
                logs: sink.lines, stats: finalStats, warnings: warnings, dump: dump)
        }
        if !drewSomething {
            return failure(
                "\(filename) drew nothing in \(ValidationSchedule.frameCount) test frames: every frame was empty or "
                    + "fully transparent. Draw something in peek.frame (\(DrawingDocs.validation))",
                logs: sink.lines, stats: finalStats, warnings: warnings, dump: dump)
        }

        let threshold = Double(ValidationSchedule.frameCount) * 0.10
        for (index, changes) in outlineChanges.sorted(by: { $0.key < $1.key }) where Double(changes) > threshold {
            warnings.append(ValidationWarning(
                code: "glass_outline_unstable",
                message: "glass fill #\(index) changed outline in \(changes)/\(ValidationSchedule.frameCount) frames. "
                    + "Glass outlines are applied at most 10 times per second, so it looks choppy and costs CPU. "
                    + "Keep glass outlines fixed and animate on top of them (\(DrawingDocs.glass))."))
        }
        if let frame = textInCompactFrame {
            warnings.append(ValidationWarning(
                code: "text_in_compact",
                message: "text is drawn in compact mode (first at frame \(frame)). The visual is tiny there: "
                    + "check input.mode and drop text and thin details (\(DrawingDocs.rules))."))
        }
        if maxDropped > 0 {
            warnings.append(ValidationWarning(
                code: "ops_truncated",
                message: "a frame recorded more than \(DrawingLimits.maxOpsPerFrame) ops (\(opsMax)); "
                    + "\(maxDropped) were ignored (\(DrawingDocs.limits)). Draw less per frame."))
        }
        if glassOverflowFrames > 0 {
            warnings.append(ValidationWarning(
                code: "glass_limit",
                message: "more than \(DrawingLimits.maxGlassFills) glass/blur fills in \(glassOverflowFrames) of "
                    + "\(ValidationSchedule.frameCount) frames; the extra ones are drawn as a flat translucent fill "
                    + "(\(DrawingDocs.limits))."))
        }

        var preview: Data?
        if options.preview {
            preview = PreviewRenderer.png(cells: previewCells, images: images)
            if preview == nil {
                warnings.append(ValidationWarning(code: "preview_failed", message: "the preview PNG could not be encoded"))
            }
        }
        return ValidationReport(ok: true, stats: finalStats, warnings: warnings, logs: sink.lines, error: nil,
                                dump: dump, previewPNG: preview)
    }

    /// The stack without prelude frames, with the offending source line and a caret.
    private func clean(_ stack: String?) -> String? {
        DrawingStack.clean(stack, source: source, filename: filename)
    }

    private func millis(_ duration: Duration) -> Int {
        let (seconds, attoseconds) = duration.components
        return Int(seconds) * 1000 + Int(attoseconds / 1_000_000_000_000_000)
    }

    private func stats(_ frames: Int, _ times: [Double], _ opsMax: Int, _ rebuilds: Int) -> ValidationStats {
        let sorted = times.sorted()
        func percentile(_ q: Double) -> Double {
            guard !sorted.isEmpty else { return 0 }
            let rank = Int((q * Double(sorted.count)).rounded(.up)) - 1
            return round3(sorted[max(0, min(sorted.count - 1, rank))])
        }
        return ValidationStats(frames: frames, p50Ms: percentile(0.5), p95Ms: percentile(0.95),
                               maxMs: round3(sorted.last ?? 0), opsMax: opsMax, glassRebuilds: rebuilds)
    }

    private func round3(_ value: Double) -> Double { (value * 1000).rounded() / 1000 }

    // swiftlint:disable:next function_parameter_count
    private func failure(_ message: String, stack: String? = nil, frame: Int? = nil, summary: String? = nil,
                         logs: [String], stats: ValidationStats? = nil, warnings: [ValidationWarning] = [],
                         dump: JSONValue? = nil) -> ValidationReport {
        ValidationReport(ok: false, stats: stats, warnings: warnings, logs: logs,
                         error: ValidationFailure(message: message, stack: stack, frame: frame, inputSummary: summary),
                         dump: dump)
    }

    static func hasVisiblePixels(_ image: CGImage) -> Bool {
        guard let data = image.dataProvider?.data, let bytes = CFDataGetBytePtr(data) else { return false }
        let length = CFDataGetLength(data)
        let alphaFirst: Bool
        switch image.alphaInfo {
        case .premultipliedFirst, .first, .noneSkipFirst: alphaFirst = image.byteOrderInfo != .order32Little
        default: alphaFirst = image.byteOrderInfo == .order32Little
        }
        // BGRA little-endian keeps alpha in byte 3; RGBA big-endian also byte 3; ARGB big-endian byte 0.
        let alphaOffset = alphaFirst ? 0 : 3
        var index = alphaOffset
        while index < length {
            if bytes[index] > 12 { return true }
            index += 4
        }
        return false
    }

    // MARK: Dump (visual.md A10)

    static func dumpNumber(_ value: Double) -> JSONValue {
        let rounded = (value * 1000).rounded() / 1000
        if rounded == rounded.rounded(), abs(rounded) < 1e15 { return .int(Int64(rounded)) }
        return .double(rounded)
    }

    static func dump(_ list: DisplayList, frame: Int, summary: String) -> JSONValue {
        var layers: [JSONValue] = []
        for layer in list.layers {
            switch layer {
            case .glass(let glass):
                let t = glass.transform
                layers.append(.object([
                    "kind": .string("glass"),
                    "path": .string(SVGPathWriter.string(glass.unitPath)),
                    "local_path": .string(SVGPathWriter.string(glass.localPath)),
                    "transform": .array([t.a, t.b, t.c, t.d, t.tx, t.ty].map { dumpNumber(Double($0)) }),
                    "rule": .string(glass.rule == .evenOdd ? "evenodd" : "nonzero"),
                    "style": .string(glass.style.cssName),
                    "tint": glass.tintCSS.map(JSONValue.string) ?? .null,
                    "interactive": .bool(glass.interactive),
                    "hash": .string(glass.hash.short("g")),
                ]))
            case .blur(let blur):
                layers.append(.object([
                    "kind": .string("blur"),
                    "path": .string(SVGPathWriter.string(blur.path)),
                    "rule": .string(blur.rule == .evenOdd ? "evenodd" : "nonzero"),
                    "material": .string(blur.material.cssName),
                    "hash": .string(blur.hash.short("b")),
                ]))
            case .draw(let draw):
                layers.append(.object([
                    "kind": .string(draw.vibrant ? "vibrant-draw" : "draw"),
                    "hash": .string(draw.hash.short(draw.vibrant ? "v" : "d")),
                    "ops": .array(draw.dumpOps ?? []),
                ]))
            }
        }
        return .object([
            "frame": .int(Int64(frame)),
            "again": .bool(list.again),
            "ops": .int(Int64(list.recordedOps)),
            "dropped_ops": .int(Int64(list.droppedOps)),
            "input": .string(summary),
            "layers": .array(layers),
        ])
    }
}

/// `--preview`: a PNG grid of test frames, glass drawn as a flat translucent approximation (visual.md B11).
enum PreviewRenderer {
    static let cell = 160
    static let caption = 22
    static let gap = 8
    static let columns = 4

    static func png(cells: [(Int, ValidationStep, DisplayList)], images: [Int: CGImage]) -> Data? {
        guard !cells.isEmpty else { return nil }
        let rows = (cells.count + columns - 1) / columns
        let width = columns * cell + (columns + 1) * gap
        let height = rows * (cell + caption) + (rows + 1) * gap
        guard let grid = CGContext(data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
                                   space: CGReplay.sRGB, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
        else { return nil }
        grid.setFillColor(CGColor(srgbRed: 0.96, green: 0.96, blue: 0.97, alpha: 1))
        grid.fill(CGRect(x: 0, y: 0, width: width, height: height))

        for (position, (index, step, list)) in cells.enumerated() {
            let column = position % columns, row = position / columns
            let x = gap + column * (cell + gap)
            let yTop = gap + row * (cell + caption + gap)
            // CG's origin is bottom-left.
            let cellRect = CGRect(x: x, y: height - yTop - cell, width: cell, height: cell)
            let backdrop = CSSColor.parse(step.input.backdrop.color) ?? .white
            grid.setFillColor(backdrop.cgColor)
            grid.fill(cellRect)
            if let image = renderCell(list, images: images) {
                grid.draw(image, in: cellRect)
            }
            let input = step.input
            let label = "#\(index) \(input.phase.rawValue) · \(input.mode.rawValue) · \(input.appearance.rawValue)"
            drawCaption(label, in: grid, at: CGPoint(x: CGFloat(x), y: CGFloat(height - yTop - cell - caption + 6)))
        }
        guard let image = grid.makeImage() else { return nil }
        let data = NSMutableData()
        guard let destination = CGImageDestinationCreateWithData(data, UTType.png.identifier as CFString, 1, nil)
        else { return nil }
        CGImageDestinationAddImage(destination, image, nil)
        guard CGImageDestinationFinalize(destination) else { return nil }
        return data as Data
    }

    static func renderCell(_ list: DisplayList, images: [Int: CGImage]) -> CGImage? {
        guard let context = CGReplay.makeContext(pixels: cell, colorSpace: CGReplay.sRGB) else { return nil }
        for layer in list.layers {
            switch layer {
            case .draw(let draw):
                CGReplay.replay(draw.commands, in: context, pixels: cell,
                                environment: ReplayEnvironment(images: images, monochrome: draw.vibrant))
            case .glass(let glass):
                CGReplay.drawFlatGlass(glass.unitPath, rule: glass.rule, tint: glass.tint, in: context)
            case .blur(let blur):
                CGReplay.drawFlatBlur(blur.path, rule: blur.rule, in: context)
            }
        }
        return context.makeImage()
    }

    private static func drawCaption(_ text: String, in context: CGContext, at origin: CGPoint) {
        let font = CTFontCreateUIFontForLanguage(.system, 11, nil)!
        let attributes: [CFString: Any] = [
            kCTFontAttributeName: font,
            kCTForegroundColorAttributeName: CGColor(srgbRed: 0.25, green: 0.25, blue: 0.28, alpha: 1),
        ]
        let string = CFAttributedStringCreate(nil, text as CFString, attributes as CFDictionary)!
        let line = CTLineCreateWithAttributedString(string)
        context.saveGState()
        context.textPosition = origin
        CTLineDraw(line, context)
        context.restoreGState()
    }
}
