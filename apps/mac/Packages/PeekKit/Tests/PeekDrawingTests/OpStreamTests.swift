import CoreGraphics
import Foundation
import PeekCore
import Testing

@testable import PeekDrawing

private func ops(_ list: DisplayList) -> [JSONValue] {
    list.layers.flatMap { layer -> [JSONValue] in
        guard case .draw(let draw) = layer else { return [.string("<\(layer.kindName)>")] }
        return draw.dumpOps ?? []
    }
}

private func frameOps(_ body: String, input: InputSnapshot = .sample()) async throws -> (EngineFrame, DisplayList) {
    let harness = try await EngineHarness()
    let status = await harness.load("peek.frame((ctx, input) => { \(body)\n return true })")
    #expect(status == .ok, "\(status)")
    let (frame, list) = await harness.decode(input, dump: true)
    return (frame, try #require(list, "\(frame.status)"))
}

@Suite("prelude op stream")
struct OpStreamTests {
    @Test("canvas calls round-trip through the Float64 op stream and string table")
    func roundTrip() async throws {
        let (frame, list) = try await frameOps("""
            ctx.save()
            ctx.translate(10, 20); ctx.rotate(0.5); ctx.scale(2, 3)
            ctx.fillStyle = 'rgba(255,255,255,0.6)'
            ctx.strokeStyle = '#1c1c1c'
            ctx.lineWidth = 1.6; ctx.lineCap = 'round'; ctx.lineJoin = 'bevel'; ctx.miterLimit = 4
            ctx.setLineDash([1, 2, 3]); ctx.lineDashOffset = 0.5
            ctx.globalAlpha = 0.5; ctx.globalCompositeOperation = 'multiply'
            ctx.shadowColor = 'black'; ctx.shadowBlur = 2; ctx.shadowOffsetX = 1; ctx.shadowOffsetY = -1
            ctx.filter = 'blur(3px)'
            ctx.font = '600 6px SF Pro'; ctx.textAlign = 'center'; ctx.textBaseline = 'middle'
            ctx.beginPath(); ctx.moveTo(1, 2); ctx.lineTo(3, 4); ctx.arc(50, 50, 10, 0, 6.283, true)
            ctx.arcTo(1, 1, 5, 5, 2); ctx.ellipse(5, 5, 2, 3, 0.1, 0, 1, false); ctx.rect(1, 2, 3, 4)
            ctx.roundRect(13, 25, 74, 10, 2); ctx.quadraticCurveTo(1, 2, 3, 4); ctx.bezierCurveTo(1, 2, 3, 4, 5, 6)
            ctx.closePath(); ctx.fill('evenodd'); ctx.stroke(); ctx.clip()
            ctx.fillRect(1, 2, 3, 4); ctx.strokeRect(1, 2, 3, 4); ctx.clearRect(1, 2, 3, 4)
            ctx.fillText('Prateek', 50, 32); ctx.strokeText('x', 1, 2, 30)
            ctx.restore()
            """)
        #expect(frame.strings == ["rgba(255,255,255,0.6)", "#1c1c1c", "black", "600 6px SF Pro", "Prateek", "x"])
        let expected: [JSONValue] = [
            ["save"], ["translate", 10, 20], ["rotate", 0.5], ["scale", 2, 3],
            ["fillStyle", "rgba(255,255,255,0.6)"], ["strokeStyle", "#1c1c1c"],
            ["lineWidth", 1.6], ["lineCap", "round"], ["lineJoin", "bevel"], ["miterLimit", 4],
            ["setLineDash", [1, 2, 3, 1, 2, 3]], ["lineDashOffset", 0.5], ["globalAlpha", 0.5],
            ["globalCompositeOperation", "multiply"], ["shadowColor", "black"], ["shadowBlur", 2],
            ["shadowOffsetX", 1], ["shadowOffsetY", -1], ["filter", "blur(3px)"], ["font", "600 6px SF Pro"],
            ["textAlign", "center"], ["textBaseline", "middle"],
            ["beginPath"], ["moveTo", 1, 2], ["lineTo", 3, 4], ["arc", 50, 50, 10, 0, 6.283, 1],
            ["arcTo", 1, 1, 5, 5, 2], ["ellipse", 5, 5, 2, 3, 0.1, 0, 1, 0], ["rect", 1, 2, 3, 4],
            ["roundRect", 13, 25, 74, 10, 2, 2, 2, 2, 2, 2, 2, 2], ["quadraticCurveTo", 1, 2, 3, 4],
            ["bezierCurveTo", 1, 2, 3, 4, 5, 6], ["closePath"], ["fill", "evenodd"], ["stroke"], ["clip", "nonzero"],
            ["fillRect", 1, 2, 3, 4], ["strokeRect", 1, 2, 3, 4], ["clearRect", 1, 2, 3, 4],
            ["fillText", "Prateek", 50, 32], ["strokeText", "x", 1, 2, 30], ["restore"],
        ]
        #expect(ops(list) == expected, "\(ops(list).map(\.jsonString))")
        #expect(list.recordedOps == expected.count)
        #expect(list.droppedOps == 0)
        #expect(list.textCount == 2)
    }

    @Test("style ops are recorded only when the value changes, and save/restore stay in sync")
    func styleDedup() async throws {
        let (_, list) = try await frameOps("""
            ctx.fillStyle = 'red'; ctx.fillStyle = 'red'; ctx.lineWidth = 1; ctx.lineWidth = 2; ctx.lineWidth = 2
            ctx.save(); ctx.fillStyle = 'blue'; ctx.restore()
            ctx.fillStyle = 'red'
            ctx.fillStyle = '#000000'
            ctx.fillRect(0, 0, 1, 1)
            """)
        let names = ops(list).compactMap { $0.arrayValue?.first?.stringValue }
        #expect(names == ["fillStyle", "lineWidth", "save", "fillStyle", "restore", "fillStyle", "fillRect"], "\(names)")
    }

    @Test("state resets every frame: nothing carries over")
    func stateResetsPerFrame() async throws {
        let harness = try await EngineHarness()
        #expect(await harness.load("""
            let n = 0
            peek.frame(ctx => {
              if (n++ === 0) { ctx.fillStyle = 'red'; ctx.translate(5, 5); ctx.save() }
              ctx.fillStyle = 'red'
              ctx.fillRect(0, 0, 1, 1)
              return ctx.getTransform().e === 0
            })
            """) == .ok)
        let (first, firstDecoded) = await harness.decode(dump: true)
        let firstList = try #require(firstDecoded)
        #expect(!first.again)
        #expect(ops(firstList).count == 4)  // fillStyle, translate, save, fillRect
        let (second, secondDecoded) = await harness.decode(dump: true)
        let secondList = try #require(secondDecoded)
        #expect(second.again)
        #expect(ops(secondList).map(\.jsonString) == [#"["fillStyle","red"]"#, #"["fillRect",0,0,1,1]"#])
    }

    @Test("Path2D accepts SVG path strings, including arcs, and is inlined where it is used")
    func path2DSVG() async throws {
        let (_, list) = try await frameOps("""
            const p = new Path2D('M10 10 H90 v80 L10 90 Z m5 5 a5 5 0 1 0 10 0 q 1 1 2 2 t 3 3 c 1 1 2 2 3 3 s 1 1 2 2')
            ctx.fill(p, 'evenodd')
            const copy = new Path2D(p)
            copy.addPath(new Path2D('M0 0 L1 1'), { a: 2, b: 0, c: 0, d: 2, e: 5, f: 5 })
            ctx.stroke(copy)
            """)
        let all = ops(list)
        let fill = try #require(all.first?.arrayValue)
        #expect(fill[0] == "fill" && fill[1] == "evenodd")
        let block = try #require(fill[2].objectValue?["path2d"]?.arrayValue)
        let names = block.compactMap { $0.arrayValue?.first?.stringValue }
        #expect(names == ["moveTo", "lineTo", "lineTo", "lineTo", "closePath", "moveTo", "ellipse", "quadraticCurveTo",
                          "quadraticCurveTo", "bezierCurveTo", "bezierCurveTo"], "\(names)")
        // H90 → lineTo(90, 10); v80 → lineTo(90, 90) (relative)
        #expect(block[1] == ["lineTo", 90, 10])
        #expect(block[2] == ["lineTo", 90, 90])
        // the arc from (15,15) to (25,15) with radius 5 is a half circle centred at (20,15)
        let ellipse = try #require(block[6].arrayValue)
        #expect(ellipse[1] == 20 && ellipse[2] == 15 && ellipse[3] == 5 && ellipse[4] == 5)
        let stroke = try #require(all.last?.arrayValue)
        #expect(stroke[0] == "stroke")
        let strokeBlock = try #require(stroke[1].objectValue?["path2d"]?.arrayValue)
        #expect(strokeBlock.count == block.count + 2)
    }

    @Test("an invalid SVG path keeps the valid prefix and logs a warning once")
    func invalidSVG() async throws {
        let harness = try await EngineHarness()
        #expect(await harness.load("""
            const p = new Path2D('M10 10 L20 20 X 5')
            peek.frame(ctx => { ctx.stroke(p); return false })
            """) == .ok)
        let (_, decoded) = await harness.decode(dump: true)
        let list = try #require(decoded)
        let block = try #require(ops(list).first?.arrayValue?[1].objectValue?["path2d"]?.arrayValue)
        #expect(block.count == 2)
        let logs = await harness.logs
        #expect(logs.count == 1)
        #expect(logs.first?.contains("SVG path data is invalid") == true, "\(logs)")
    }

    @Test("gradients are recorded inline with their stops")
    func gradients() async throws {
        let (_, list) = try await frameOps("""
            const g = ctx.createLinearGradient(0, 0, 100, 0)
            g.addColorStop(1, 'blue'); g.addColorStop(0, 'red')
            ctx.fillStyle = g
            ctx.fillStyle = g
            const r = ctx.createRadialGradient(50, 50, 0, 50, 50, 40)
            r.addColorStop(0, '#fff')
            ctx.strokeStyle = r
            const c = ctx.createConicGradient(0.5, 50, 50)
            ctx.fillStyle = c
            ctx.fillRect(0, 0, 1, 1)
            """)
        let all = ops(list)
        try #require(all.count == 4)
        let linear = try #require(all[0].arrayValue?[1].objectValue)
        #expect(linear["type"] == "linear")
        #expect(linear["params"] == [0, 0, 100, 0])
        #expect(linear["stops"] == [[0, "#ff0000"], [1, "#0000ff"]])
        #expect(all[1].arrayValue?[1].objectValue?["type"] == "radial")
        #expect(all[2].arrayValue?[1].objectValue?["params"] == [0.5, 50, 50])
    }

    @Test("invalid values are ignored like canvas does, unsupported calls throw precise errors")
    func invalidValues() async throws {
        let harness = try await EngineHarness()
        #expect(await harness.load("""
            peek.frame(ctx => {
              ctx.lineWidth = -1; ctx.lineWidth = NaN; ctx.globalAlpha = 2; ctx.lineCap = 'wiggly'
              ctx.globalCompositeOperation = 'xor'; ctx.moveTo(NaN, 1); ctx.filter = 'sepia(1)'
              ctx.fillRect(Infinity, 0, 1, 1)
              let errors = []
              for (const f of [() => ctx.getImageData(0, 0, 1, 1), () => ctx.arc(0, 0, -1, 0, 1),
                               () => ctx.fill('sideways'), () => ctx.drawImage({}, 0, 0),
                               () => ctx.drawImage({ id: 1, width: 2, height: 2 }, 0, 0, 1),
                               () => ctx.fillGlass({ style: 'shiny' }), () => ctx.roundRect(0, 0, 1, 1, [1, 2, 3, 4, 5])]) {
                try { f() } catch (e) { errors.push(e.name + ': ' + e.message) }
              }
              peek.log(errors.join('\\n'))
              return false
            })
            """) == .ok)
        let (frame, decoded) = await harness.decode(dump: true)
        let list = try #require(decoded)
        #expect(frame.status == .ok)
        #expect(ops(list).isEmpty)
        let logs = await harness.logs
        let errors = try #require(logs.last).split(separator: "\n").map(String.init)
        #expect(errors == [
            "TypeError: ctx.getImageData is not supported in peek drawings: drawings cannot read pixels (visual.md A5)",
            "RangeError: arc: the radius -1 is negative (canvas IndexSizeError)",
            "TypeError: fill: the fill rule must be 'nonzero' or 'evenodd', got \"sideways\"",
            "TypeError: drawImage: expected an image handle from input.show or input.ask, got object",
            "TypeError: drawImage: takes 3, 5 or 9 arguments, got 4",
            "TypeError: fillGlass: style must be 'regular' or 'clear', got \"shiny\"",
            "RangeError: roundRect: expected 1 to 4 radii, got 5",
        ], "\(errors)")
        #expect(logs.contains { $0.contains("lineCap \"wiggly\" is not supported") })
        #expect(logs.contains { $0.contains("globalCompositeOperation \"xor\" is not supported") })
        #expect(logs.contains { $0.contains("filter \"sepia(1)\" is not supported") })
    }

    @Test("the recorder's natives are unreachable and peek cannot be replaced")
    func sandbox() async throws {
        let harness = try await EngineHarness()
        let status = await harness.load("""
            'use strict'
            for (const name of ['__peek_ops', '__peek_flush', '__peek_log', '__peek_measure', '__peek_frame',
                                '__peek_event', 'setTimeout', 'fetch', 'require', 'Date']) {
              if (typeof globalThis[name] !== 'undefined') throw new Error(name + ' is reachable')
            }
            try { peek = null; throw new Error('peek was replaced') } catch (e) { if (!(e instanceof TypeError)) throw e }
            try { peek.frame = null; throw new Error('peek.frame was replaced') } catch (e) { if (!(e instanceof TypeError)) throw e }
            if (typeof Path2D !== 'function' || typeof CanvasGradient !== 'function') throw new Error('missing classes')
            peek.frame(() => false)
            """)
        #expect(status == .ok, "\(status)")
    }

    @Test("ctx calls outside peek.frame are ignored with one warning")
    func outsideFrame() async throws {
        let harness = try await EngineHarness()
        #expect(await harness.load("""
            let saved = null
            peek.on('click', () => { saved.fillRect(0, 0, 1, 1); saved.fillRect(0, 0, 1, 1) })
            peek.frame(ctx => { saved = ctx; return false })
            """) == .ok)
        _ = await harness.decode()
        #expect(await harness.event(.click(x: 1, y: 2, count: 1)) == .ok)
        let logs = await harness.logs
        #expect(logs == ["warning: ctx was used outside peek.frame(); drawing calls only count inside the frame callback"])
    }

    @Test("events reach handlers with their payloads; 'word' is accepted but never fires")
    func events() async throws {
        let harness = try await EngineHarness()
        #expect(await harness.load("""
            peek.on('word', () => peek.log('never'))
            peek.on('click', e => peek.log('click', e.x, e.y, e.count))
            peek.on('move', e => peek.log('move', e.from, e.to))
            peek.on('answer', e => peek.log('answer', JSON.stringify(e.value), e.via))
            peek.on('enter', e => peek.log('enter', typeof e))
            peek.frame(() => false)
            """) == .ok)
        #expect(await harness.event(.click(x: 12.5, y: 40, count: 2)) == .ok)
        #expect(await harness.event(.move(from: .bottom, to: .right)) == .ok)
        #expect(await harness.event(.answer(value: .range(lower: 1, upper: 2), via: .voice)) == .ok)
        #expect(await harness.event(.enter) == .ok)
        let logs = await harness.logs
        #expect(logs == [
            "warning: peek.on('word') never fires: peek has no word timing (use input.speech.progress and input.speech.level)",
            "click 12.5 40 2", "move 5 3", "answer [1,2] voice", "enter undefined",
        ], "\(logs)")
    }

    @Test("unknown event names are refused with the list of events")
    func unknownEvent() async throws {
        let harness = try await EngineHarness()
        let status = await harness.load("peek.on('hover', () => {})")
        guard case .threw(let message, _) = status else {
            Issue.record("expected a throw, got \(status)")
            return
        }
        #expect(message == "TypeError: peek.on: unknown event \"hover\"; events are enter, leave, send, answer, click, move")
    }

    @Test("measureText returns Core Text metrics that follow the font and the alignment")
    func measureText() async throws {
        let harness = try await EngineHarness()
        #expect(await harness.load("""
            peek.frame(ctx => {
              ctx.font = '10px SF Pro'
              const a = ctx.measureText('Hello')
              ctx.font = '20px SF Pro'
              const b = ctx.measureText('Hello')
              ctx.textAlign = 'center'
              const c = ctx.measureText('Hello')
              peek.log(JSON.stringify([a.width, b.width, c.actualBoundingBoxLeft - b.actualBoundingBoxLeft,
                                       a.fontBoundingBoxAscent, a.actualBoundingBoxAscent]))
              return false
            })
            """) == .ok)
        _ = await harness.decode()
        let line = try #require(await harness.logs.last)
        let parsed = try StrictJSON.parse(Data(line.utf8))
        let values = try #require(parsed.arrayValue).compactMap(\.doubleValue)
        #expect(values.count == 5)
        #expect(values[0] > 15 && values[0] < 40, "\(values)")
        // SF Pro's optical sizes make larger text slightly tighter than a linear scale.
        #expect(values[1] > 1.6 * values[0] && values[1] <= 2.05 * values[0], "\(values)")
        #expect(abs(values[2] - values[1] / 2) < 0.01)
        #expect(values[3] > 5 && values[4] > 5)
    }

    @Test("the return value controls again; a returned Promise counts as false")
    func againFlag() async throws {
        let harness = try await EngineHarness()
        #expect(await harness.load("""
            let n = 0
            peek.frame(ctx => { n++; if (n === 1) return 1; if (n === 2) return undefined; return Promise.resolve(true) })
            """) == .ok)
        #expect(await harness.frame().again)
        #expect(!(await harness.frame().again))
        #expect(!(await harness.frame().again))
        #expect(await harness.logs == ["warning: peek.frame callbacks must be synchronous; a returned Promise counts as false"])
    }

    @Test("runaway recursion is a catchable RangeError, not a crash (1 MiB JS stack on a 4 MiB thread)")
    func deepRecursion() async throws {
        let harness = try await EngineHarness()
        #expect(await harness.load("""
            function down(n) { return down(n + 1) + 1 }
            peek.frame(ctx => {
              try { down(0) } catch (e) { peek.log(e.name + ': ' + e.message) }
              ctx.fillRect(0, 0, 1, 1)
              return false
            })
            """) == .ok)
        let frame = await harness.frame(budget: .seconds(5))
        #expect(frame.status == .ok, "\(frame.status)")
        let logs = await harness.logs
        #expect(logs.last?.hasPrefix("RangeError") == true, "\(logs)")
    }

    @Test("drawImage records the handle and resolves the 3, 5 and 9 argument forms")
    func drawImageForms() async throws {
        let (_, list) = try await frameOps("""
            const img = { id: 7, width: 40, height: 20 }
            ctx.drawImage(img, 1, 2)
            ctx.drawImage(img, 1, 2, 3, 4)
            ctx.drawImage(img, 5, 6, 7, 8, 1, 2, 3, 4)
            """)
        #expect(ops(list).map(\.jsonString) == [
            #"["drawImage",{"image":7},0,0,40,20,1,2,40,20]"#,
            #"["drawImage",{"image":7},0,0,40,20,1,2,3,4]"#,
            #"["drawImage",{"image":7},5,6,7,8,1,2,3,4]"#,
        ])
    }
}
