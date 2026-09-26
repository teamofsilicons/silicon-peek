import Foundation
import ImageIO
import PeekCore
import Testing

@testable import PeekDrawing

private let key = SiliconKey(context: .production, orgID: "tos", actorID: "si:dj")

private func script(_ source: String, filename: String = "drawing.js") -> DrawingScript {
    DrawingScript(key: key, sha256: "0", source: Data(source.utf8), filename: filename)
}

/// Serialized: frame budgets are wall-clock, and QuickJS runs unoptimised in debug builds, so parallel
/// validations would measure CPU contention rather than the drawings.
@Suite("A9 validation", .serialized)
struct ValidatorTests {
    @Test("every sample drawing validates without errors", arguments: SampleDrawings.all)
    func samplesValidate(sample: SampleDrawings.Sample) async throws {
        let report = await DrawingValidator.validate(script(sample.source, filename: sample.filename))
        #expect(report.ok, "\(sample.filename): \(report.error?.message ?? "") \(report.error?.stack ?? "")")
        #expect(report.error == nil)
        let stats = try #require(report.stats)
        // A9 tolerates a few overruns (only p95 counts); a busy test machine can preempt a frame.
        #expect(stats.frames >= 86, "\(stats)")
        #expect(stats.p95Ms < 4)
        #expect(stats.opsMax > 0 && stats.opsMax < DrawingLimits.maxOpsPerFrame)
        // The samples follow A6: no unstable glass, no text in compact, nothing ignored.
        #expect(report.warnings.isEmpty, "\(sample.filename): \(report.warnings)")
        #expect(!report.logs.contains { $0.hasPrefix("warning:") }, "\(report.logs)")
    }

    @Test("the cassette's glass body is stable: rebuilt only when the tint changes per send")
    func cassetteGlassStable() async throws {
        let report = await DrawingValidator.validate(script(SampleDrawings.cassette.source, filename: "cassette.js"))
        let stats = try #require(report.stats)
        // Built once, rebuilt when the cover art (tint) disappears at frame 60: 2 builds, never per frame.
        #expect(stats.glassRebuilds >= 1 && stats.glassRebuilds <= 3, "\(stats.glassRebuilds)")
        #expect(!report.warnings.contains { $0.code == "glass_outline_unstable" })
    }

    @Test("a script that throws at frame 14 fails with the frame, the stack and the input")
    func throwsAtFrame14() async throws {
        let report = await DrawingValidator.validate(script("""
            let frame = 0
            peek.frame((ctx, input) => {
              ctx.fillRect(0, 0, 10, 10)
              if (frame++ === 14) {
                const art = input.show.elements.find(e => e.type === 'missing')
                return art.colors.dominant
              }
              return true
            })
            """, filename: "cassette.js"))
        #expect(!report.ok)
        let error = try #require(report.error)
        #expect(error.frame == 14)
        // The CLI prints "✗ frame 14 threw: <message>": the message itself never repeats that prefix.
        #expect(error.message.hasPrefix("TypeError"), "\(error.message)")
        #expect(!error.message.contains("threw"), "\(error.message)")
        let stack = try #require(error.stack)
        #expect(stack.hasPrefix("at cassette.js:6:"), "\(stack)")
        #expect(!stack.contains("peek-prelude.js"), "\(stack)")
        // The offending line with a caret under the failing expression (docs/drawing.md "Validation").
        let lines = stack.split(separator: "\n").map(String.init)
        #expect(lines.count >= 3, "\(stack)")
        #expect(lines[1] == "  return art.colors.dominant", "\(stack)")
        let caret = try #require(lines[2].firstIndex(of: "^"))
        let column = lines[2].distance(from: lines[2].startIndex, to: caret)
        #expect(["a", "."].contains(lines[1][lines[1].index(lines[1].startIndex, offsetBy: column)]), "\(stack)")
        #expect(error.inputSummary?.contains("phase=showing") == true, "\(error.inputSummary ?? "")")
        #expect(report.stats?.frames == 14)
    }

    @Test("an endless loop is interrupted and fails validation")
    func infiniteLoop() async throws {
        let clock = ContinuousClock()
        let started = clock.now
        let report = await DrawingValidator.validate(script("""
            peek.frame(() => { for (;;) {} })
            """))
        #expect(clock.now - started < .seconds(5))
        #expect(!report.ok)
        let error = try #require(report.error)
        #expect(error.message.hasPrefix("frame 4 was interrupted"), "\(error.message)")
        #expect(error.message.contains("4 ms"))
        #expect(!error.message.contains("visual.md"))
        #expect(error.message.contains("https://peek.teamofsilicons.com/docs/drawing#limits"), "\(error.message)")
        // Not a throw: `frame` stays unset so no "frame 4 threw:" prefix is added; the input is still reported.
        #expect(error.frame == nil)
        #expect(error.inputSummary?.contains("phase=entering") == true, "\(error.inputSummary ?? "")")
    }

    @Test("an endless loop at the top level is stopped at load")
    func topLevelLoop() async throws {
        let report = await DrawingValidator.validate(script("while (true) {}"))
        #expect(!report.ok)
        #expect(report.error?.message.contains("top-level code") == true, "\(report.error?.message ?? "")")
    }

    @Test("a memory bomb hits the 16 MB limit and fails")
    func memoryBomb() async throws {
        let report = await DrawingValidator.validate(script("""
            const keep = []
            peek.frame(() => { for (let i = 0; i < 200; i++) keep.push(new Array(100000).fill(i)); return true })
            """))
        #expect(!report.ok)
        let message = try #require(report.error?.message)
        #expect(message.contains("16 MB"), "\(message)")
    }

    @Test("a script that draws nothing fails")
    func drawsNothing() async throws {
        let report = await DrawingValidator.validate(script("""
            peek.frame((ctx, input) => {
              ctx.fillStyle = 'rgba(0,0,0,0)'
              ctx.fillRect(0, 0, 100, 100)
              ctx.beginPath()
              return false
            })
            """))
        #expect(!report.ok)
        #expect(report.error?.message.contains("drew nothing") == true, "\(report.error?.message ?? "")")
    }

    @Test("a script that never calls peek.frame fails")
    func noFrameFunction() async throws {
        let report = await DrawingValidator.validate(script("const x = 1"))
        #expect(!report.ok)
        #expect(report.error?.message.contains("peek.frame") == true, "\(report.error?.message ?? "")")
    }

    @Test("a syntax error fails at load with the line")
    func syntaxError() async throws {
        let report = await DrawingValidator.validate(script("peek.frame((ctx) => {\n  ctx.fill(\n})", filename: "bad.js"))
        #expect(!report.ok)
        let error = try #require(report.error)
        #expect(error.message.hasPrefix("loading bad.js threw: SyntaxError"), "\(error.message)")
        #expect(error.stack?.contains("bad.js:") == true, "\(error.stack ?? "")")
    }

    @Test("scripts over 256 KB are rejected before running")
    func tooLarge() async throws {
        let big = "// " + String(repeating: "x", count: 300 * 1024) + "\npeek.frame(() => false)"
        let report = await DrawingValidator.validate(script(big, filename: "big.js"))
        #expect(!report.ok)
        #expect(report.error?.message.contains("256 KB") == true)
    }

    @Test("a throwing click handler fails validation")
    func throwingHandler() async throws {
        let report = await DrawingValidator.validate(script("""
            peek.on('click', e => { throw new Error('boom ' + e.count) })
            peek.frame(ctx => { ctx.fillRect(10, 10, 10, 10); return true })
            """))
        #expect(!report.ok)
        #expect(report.error?.message == "Error: boom 1 (in the 'click' handler)")
        #expect(report.error?.frame == 25)
    }

    @Test("glass that changes outline every frame is accepted with a warning")
    func unstableGlass() async throws {
        let report = await DrawingValidator.validate(script("""
            peek.frame((ctx, input) => {
              ctx.beginPath()
              ctx.arc(50, 50, 30 + Math.sin(input.t * 20) * 5, 0, Math.PI * 2)
              ctx.fillGlass()
              return true
            })
            """))
        #expect(report.ok, "\(report.error?.message ?? "")")
        let warning = try #require(report.warnings.first { $0.code == "glass_outline_unstable" })
        #expect(warning.message.hasPrefix("glass fill #1 changed outline in 89/90 frames"), "\(warning.message)")
        #expect((report.stats?.glassRebuilds ?? 0) >= 89)
    }

    @Test("rotating glass through the transform is stable (the CTM is not part of the outline)")
    func rotatingGlassIsStable() async throws {
        let report = await DrawingValidator.validate(script("""
            const shape = new Path2D('M-20 -10 H20 V10 H-20 Z')
            peek.frame((ctx, input) => {
              ctx.translate(50, 50)
              ctx.rotate(input.t * 3)
              ctx.fillGlass(shape, { style: 'clear' })
              return true
            })
            """))
        #expect(report.ok, "\(report.error?.message ?? "")")
        #expect(!report.warnings.contains { $0.code == "glass_outline_unstable" }, "\(report.warnings)")
        #expect(report.stats?.glassRebuilds == 1)
    }

    @Test("text in compact mode is accepted with a warning")
    func textInCompact() async throws {
        let report = await DrawingValidator.validate(script("""
            peek.frame((ctx, input) => { ctx.font = '8px SF Pro'; ctx.fillText('hi', 10, 50); return false })
            """))
        #expect(report.ok, "\(report.error?.message ?? "")")
        let warning = try #require(report.warnings.first { $0.code == "text_in_compact" })
        #expect(warning.message.contains("frame 15"), "\(warning.message)")
    }

    @Test("more than 5,000 ops are truncated with a warning")
    func opsCap() async throws {
        // A 5,200-segment Path2D inlines as 5,203 ops, so the stroke is dropped whole.
        let report = await DrawingValidator.validate(script("""
            const zigzag = new Path2D()
            for (let i = 0; i < 5200; i++) zigzag.lineTo(i % 100, (i % 7) * 10)
            peek.frame(ctx => { ctx.fillRect(0, 0, 10, 10); ctx.stroke(zigzag); return false })
            """))
        #expect(report.ok, "\(report.error?.message ?? "")")
        let warning = try #require(report.warnings.first { $0.code == "ops_truncated" })
        #expect(warning.message.contains("5203 were ignored"), "\(warning.message)")
        #expect(report.stats?.opsMax == 5204)
    }

    @Test("a fourth glass fill is drawn flat, with a warning")
    func glassCap() async throws {
        let report = await DrawingValidator.validate(script("""
            peek.frame(ctx => {
              for (let i = 0; i < 4; i++) { ctx.beginPath(); ctx.rect(i * 25, 0, 20, 20); ctx.fillGlass() }
              return false
            })
            """))
        #expect(report.ok, "\(report.error?.message ?? "")")
        // One warning for one problem: glass_limit, not also an ignored_value about the same fills.
        let glass = report.warnings.filter { $0.message.contains("glass/blur fills") }
        #expect(glass.map(\.code) == ["glass_limit"], "\(report.warnings)")
        #expect(glass.first?.message.contains("https://peek.teamofsilicons.com/docs/drawing#limits") == true)
    }

    @Test("an unsupported globalCompositeOperation is an ignored_value warning, not a log line")
    func unsupportedCompositeOperation() async throws {
        let report = await DrawingValidator.validate(script("""
            peek.frame(ctx => {
              ctx.globalCompositeOperation = 'xor'
              ctx.fillRect(0, 0, 10, 10)
              return false
            })
            """))
        #expect(report.ok, "\(report.error?.message ?? "")")
        let ignored = report.warnings.filter { $0.code == "ignored_value" }.map(\.message)
        #expect(ignored.contains { $0.contains("globalCompositeOperation") && $0.contains("xor") }, "\(report.warnings)")
        #expect(!report.logs.contains { $0.contains("globalCompositeOperation") }, "\(report.logs)")
    }

    @Test("warnings cite the public docs, never internal spec sections")
    func publicDocLinks() async throws {
        let report = await DrawingValidator.validate(script("""
            peek.frame((ctx, input) => {
              ctx.beginPath()
              ctx.arc(50, 50, 20 + 10 * Math.sin(input.t * 20), 0, Math.PI * 2)
              ctx.fillGlass()
              ctx.font = '8px system-ui'
              ctx.fillText('hi', 10, 10)
              return true
            })
            """))
        #expect(report.ok, "\(report.error?.message ?? "")")
        #expect(!report.warnings.isEmpty)
        for warning in report.warnings {
            #expect(!warning.message.contains("visual.md"), "\(warning.message)")
        }
        let outline = try #require(report.warnings.first { $0.code == "glass_outline_unstable" })
        #expect(outline.message.contains("https://peek.teamofsilicons.com/docs/drawing#glass-what-is-cheap-and-what-is-not"))
        let compact = try #require(report.warnings.first { $0.code == "text_in_compact" })
        #expect(compact.message.contains("https://peek.teamofsilicons.com/docs/drawing#rules-for-good-drawings"))
    }

    @Test("unknown colours and fonts are reported as warnings, and peek.log reaches the logs")
    func ignoredValuesAndLogs() async throws {
        let report = await DrawingValidator.validate(script("""
            peek.log('loaded', { v: 1 })
            peek.frame(ctx => {
              ctx.fillStyle = 'blurple'
              ctx.font = '8px Comic Sans'
              ctx.fillRect(0, 0, 10, 10)
              return false
            })
            """))
        #expect(report.ok)
        #expect(report.logs.first == #"loaded {"v":1}"#)
        let messages = report.warnings.filter { $0.code == "ignored_value" }.map(\.message)
        #expect(messages.contains { $0.contains("\"blurple\" is not a CSS color") }, "\(messages)")
        #expect(messages.contains { $0.contains("\"Comic Sans\" is not available") }, "\(messages)")
    }

    @Test("--preview returns a PNG grid of test frames")
    func preview() async throws {
        let report = await DrawingValidator.validate(script(SampleDrawings.deck.source, filename: "deck.js"),
                                                     options: ValidationOptions(preview: true))
        #expect(report.ok)
        let png = try #require(report.previewPNG)
        #expect(png.starts(with: [0x89, 0x50, 0x4E, 0x47]))
        let source = try #require(CGImageSourceCreateWithData(png as CFData, nil))
        let image = try #require(CGImageSourceCreateImageAtIndex(source, 0, nil))
        #expect(image.width == 4 * 160 + 5 * 8)
        #expect(image.height == 3 * (160 + 22) + 4 * 8)
    }

    @Test("the report encodes as the drawing.validate reply (§1.6)")
    func replyShape() async throws {
        let report = await DrawingValidator.validate(script(SampleDrawings.minimalDot.source))
        let json = try StrictJSON.parse(try JSONEncoder().encode(report))
        let object = try #require(json.objectValue)
        #expect(Set(object.keys) == ["ok", "stats", "warnings", "logs", "error"])
        let stats = try #require(object["stats"]?.objectValue)
        #expect(Set(stats.keys) == ["frames", "p50_ms", "p95_ms", "max_ms", "ops_max", "glass_rebuilds"])
    }

    @Test("the runtime reads the staged file and reports unreadable paths precisely")
    @MainActor
    func runtimeValidatesFiles() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("peek-drawing-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let file = directory.appendingPathComponent("eye.js")
        try Data(SampleDrawings.eye.source.utf8).write(to: file)
        let runtime = DrawingRuntime(glassMode: .live)
        let ok = await runtime.validate(scriptAt: file, options: ValidationOptions())
        #expect(ok.ok, "\(ok.error?.message ?? "")")

        let missing = await runtime.validate(scriptAt: directory.appendingPathComponent("gone.js"), options: ValidationOptions())
        #expect(!missing.ok)
        #expect(missing.error?.message.contains("gone.js") == true)
    }
}
