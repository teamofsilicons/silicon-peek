import CoreGraphics
import Foundation
import PeekCore
import Testing

@testable import PeekDrawing

private let key = SiliconKey(context: .production, orgID: "tos", actorID: "si:dj")

@Suite("--dump-frame")
struct DumpTests {
    @Test("the dump has the visual.md A10 shape: frame, again and ordered layers")
    func cassetteDump() async throws {
        let script = DrawingScript(key: key, sha256: "0", source: SampleDrawings.cassette.data, filename: "cassette.js")
        let report = await DrawingValidator.validate(script, options: ValidationOptions(dumpFrame: 30))
        #expect(report.ok)
        let dump = try #require(report.dump?.objectValue)
        #expect(dump["frame"] == 30)
        #expect(dump["again"]?.boolValue != nil)
        #expect(dump["input"]?.stringValue?.contains("phase=asking") == true)
        let layers = try #require(dump["layers"]?.arrayValue)
        #expect(layers.count == 2)

        let glass = try #require(layers[0].objectValue)
        #expect(glass["kind"] == "glass")
        #expect(glass["rule"] == "evenodd")
        #expect(glass["style"] == "clear")
        #expect(glass["interactive"] == true)
        #expect(glass["tint"] == "rgba(200, 53, 43, 0.3)")
        #expect(glass["transform"] == [1, 0, 0, 1, 0, 0])
        let path = try #require(glass["path"]?.stringValue)
        #expect(path.hasPrefix("M12 21 L88 21 C"), "\(path)")
        #expect(path.filter { $0 == "M" }.count == 3)  // the body and two reel holes
        let hash = try #require(glass["hash"]?.stringValue)
        #expect(hash.wholeMatch(of: /g:[0-9a-f]{6}/) != nil, "\(hash)")

        let draw = try #require(layers[1].objectValue)
        #expect(draw["kind"] == "draw")
        #expect(draw["hash"]?.stringValue?.wholeMatch(of: /d:[0-9a-f]{6}/) != nil)
        let ops = try #require(draw["ops"]?.arrayValue).map(\.jsonString)
        #expect(Array(ops.prefix(8)) == [
            #"["fillStyle","rgba(255,255,255,0.6)"]"#, #"["beginPath"]"#, #"["roundRect",13,25,74,10,2,2,2,2,2,2,2,2]"#,
            #"["fill","nonzero"]"#, ##"["fillStyle","#1c1c1c"]"##, #"["font","600 6px SF Pro"]"#,
            #"["textAlign","center"]"#, #"["fillText","SIDE A",50,32]"#,
        ], "\(ops.prefix(8))")
        #expect(ops.contains(#"["clip","nonzero"]"#))
        #expect(ops.contains(#"["lineCap","round"]"#))
        #expect(ops.contains { $0.hasPrefix(#"["rotate","#) })
    }

    @Test("a transformed glass dump shows the flattened path, the local path and the transform")
    func transformedGlass() async throws {
        let script = DrawingScript(key: key, sha256: "0", source: Data("""
            const shape = new Path2D('M-10 -10 H10 V10 H-10 Z')
            peek.frame(ctx => { ctx.translate(50, 40); ctx.fillGlass(shape, { tint: 'white' }); ctx.fillRect(0, 0, 1, 1); return false })
            """.utf8), filename: "g.js")
        let report = await DrawingValidator.validate(script, options: ValidationOptions(dumpFrame: 0))
        let glass = try #require(report.dump?.objectValue?["layers"]?.arrayValue?.first?.objectValue)
        #expect(glass["local_path"] == "M-10 -10 L10 -10 L10 10 L-10 10 Z")
        #expect(glass["path"] == "M40 30 L60 30 L60 50 L40 50 Z")
        #expect(glass["transform"] == [1, 0, 0, 1, 50, 40])
        #expect(glass["tint"] == "white")
        #expect(glass["style"] == "regular")
    }

    @Test("an out-of-range frame number is a warning, not a failure")
    func outOfRange() async throws {
        let script = DrawingScript(key: key, sha256: "0", source: SampleDrawings.eye.data, filename: "eye.js")
        let report = await DrawingValidator.validate(script, options: ValidationOptions(dumpFrame: 90))
        #expect(report.ok)
        #expect(report.dump == nil)
        #expect(report.warnings.contains { $0.code == "dump_frame_unavailable" })
    }

    @Test("SVG path data is compact and rounded to 3 decimals")
    func svgWriter() {
        let path = CGMutablePath()
        path.move(to: CGPoint(x: 1.23456, y: -0.0001))
        path.addLine(to: CGPoint(x: 10, y: 20))
        path.addQuadCurve(to: CGPoint(x: 3, y: 4), control: CGPoint(x: 1, y: 2))
        path.addCurve(to: CGPoint(x: 5, y: 6), control1: CGPoint(x: 1, y: 2), control2: CGPoint(x: 3.5, y: 4))
        path.closeSubpath()
        #expect(SVGPathWriter.string(path) == "M1.235 0 L10 20 Q1 2 3 4 C1 2 3.5 4 5 6 Z")
    }
}
