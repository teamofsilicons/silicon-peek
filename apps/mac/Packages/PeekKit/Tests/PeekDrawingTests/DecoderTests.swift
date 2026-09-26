import CoreGraphics
import Foundation
import PeekCore
import Testing

@testable import PeekDrawing

private func decode(_ script: String, frames: Int = 1, input: InputSnapshot = .sample()) async throws -> [DisplayList] {
    let harness = try await EngineHarness()
    let status = await harness.load(script)
    #expect(status == .ok, "\(status)")
    var lists: [DisplayList] = []
    for index in 0..<frames {
        var snapshot = input
        snapshot.t = Double(index) / 60
        let (frame, list) = await harness.decode(snapshot, dump: true)
        lists.append(try #require(list, "\(frame.status)"))
    }
    return lists
}

@Suite("op decoder")
struct DecoderTests {
    @Test("fillGlass, fillBlur and vibrant changes split the frame into layers in op order")
    func layerSplit() async throws {
        let list = try await decode("""
            peek.frame(ctx => {
              ctx.fillRect(0, 0, 10, 10)
              ctx.beginPath(); ctx.arc(50, 50, 40, 0, Math.PI * 2); ctx.fillGlass({ tint: '#ff0000', interactive: true })
              ctx.fillRect(10, 10, 10, 10)
              ctx.vibrant = true
              ctx.fillRect(20, 20, 10, 10)
              ctx.vibrant = false
              ctx.fillBlur(new Path2D('M0 0 H10 V10 Z'), { material: 'sidebar' })
              ctx.fillRect(30, 30, 10, 10)
              return false
            })
            """)[0]
        #expect(list.layers.map(\.kindName) == ["draw", "glass", "draw", "vibrant-draw", "blur", "draw"])
        guard case .glass(let glass) = list.layers[1], case .blur(let blur) = list.layers[4] else {
            Issue.record("unexpected layers")
            return
        }
        #expect(glass.tint == RGBA(red: 1, green: 0, blue: 0, alpha: 1))
        #expect(glass.interactive)
        #expect(glass.style == .regular)
        #expect(blur.material == .sidebar)
        #expect(list.paintCount == 6)
    }

    @Test("glass records the local path and the CTM, not a flattened path (§0.1 item 6)")
    func glassLocalPath() async throws {
        let lists = try await decode("""
            const shape = new Path2D()
            shape.rect(-10, -5, 20, 10)
            peek.frame((ctx, input) => {
              ctx.translate(50, 50)
              ctx.rotate(input.t * 60)
              ctx.fillGlass(shape, { style: 'clear' })
              ctx.beginPath(); ctx.rect(-5, -5, 10, 10); ctx.fillGlass()
              return true
            })
            """, frames: 2)
        guard case .glass(let first) = lists[0].layers[0], case .glass(let second) = lists[1].layers[0],
              case .glass(let current) = lists[1].layers[1]
        else {
            Issue.record("expected glass layers")
            return
        }
        #expect(first.localPath.boundingBox == CGRect(x: -10, y: -5, width: 20, height: 10))
        #expect(first.transform.tx == 50 && first.transform.ty == 50)
        #expect(first.outlineHash == second.outlineHash)
        #expect(first.transform != second.transform)
        #expect(first.style == .clear)
        // The current path was built under one transform: also local + CTM.
        #expect(current.localPath.boundingBox == CGRect(x: -5, y: -5, width: 10, height: 10))
        #expect(current.transform == second.transform)
        #expect(current.contains(unitPoint: CGPoint(x: 50, y: 50)))
        #expect(!current.contains(unitPoint: CGPoint(x: 70, y: 50)))
    }

    @Test("a current path built under different transforms is flattened with the identity")
    func glassMixedTransforms() async throws {
        let list = try await decode("""
            peek.frame(ctx => {
              ctx.beginPath(); ctx.moveTo(0, 0); ctx.translate(10, 10); ctx.lineTo(10, 0); ctx.lineTo(0, 10)
              ctx.fillGlass()
              return false
            })
            """)[0]
        guard case .glass(let glass) = list.layers[0] else {
            Issue.record("expected a glass layer")
            return
        }
        #expect(glass.transform == .identity)
        #expect(glass.localPath.boundingBox == CGRect(x: 0, y: 0, width: 20, height: 20))
    }

    @Test("layer hashes are stable for identical frames and independent between layers")
    func hashes() async throws {
        let lists = try await decode("""
            peek.frame((ctx, input) => {
              ctx.fillStyle = 'red'; ctx.fillRect(0, 0, 10, 10)
              ctx.beginPath(); ctx.arc(50, 50, 30, 0, 7); ctx.fillGlass()
              ctx.fillStyle = 'blue'; ctx.fillRect(input.t > 0.02 ? 20 : 10, 10, 10, 10)
              return true
            })
            """, frames: 3)
        func hashes(_ list: DisplayList) -> [Hash64] {
            list.layers.map { layer in
                switch layer {
                case .draw(let draw): draw.hash
                case .glass(let glass): glass.hash
                case .blur(let blur): blur.hash
                }
            }
        }
        let (a, b, c) = (hashes(lists[0]), hashes(lists[1]), hashes(lists[2]))
        #expect(a == b)  // t = 0 and t = 1/60: identical frames
        #expect(a[0] == c[0])  // the layer before the glass did not change
        #expect(a[1] == c[1])  // neither did the glass
        #expect(a[2] != c[2])  // the moved rectangle did
    }

    @Test("the style state at a layer break carries into the next layer (and its hash)")
    func stateAcrossBreaks() async throws {
        let list = try await decode("""
            peek.frame(ctx => {
              ctx.save(); ctx.translate(10, 0); ctx.fillStyle = 'green'
              ctx.beginPath(); ctx.rect(0, 0, 5, 5); ctx.fillGlass()
              ctx.fillRect(0, 0, 5, 5)
              ctx.restore()
              ctx.fillRect(0, 0, 5, 5)
              return false
            })
            """)[0]
        guard case .draw(let draw) = list.layers.last else {
            Issue.record("expected a draw layer")
            return
        }
        #expect(draw.commands.count == 2)
        guard case .fill(let first, _) = draw.commands[0].kind, case .fill(let second, _) = draw.commands[1].kind else {
            Issue.record("expected fills")
            return
        }
        #expect(first.boundingBox.minX == 10)
        #expect(second.boundingBox.minX == 0)
        #expect(draw.commands[0].style.fill == .color(RGBA(red: 0, green: 128.0 / 255, blue: 0, alpha: 1)))
        #expect(draw.commands[1].style.fill == .color(.black))
    }

    @Test("clips intersect, follow save/restore and use unit-space paths")
    func clips() async throws {
        let list = try await decode("""
            peek.frame(ctx => {
              ctx.save()
              ctx.beginPath(); ctx.rect(0, 0, 50, 50); ctx.clip()
              ctx.translate(10, 10); ctx.beginPath(); ctx.rect(0, 0, 10, 10); ctx.clip('evenodd')
              ctx.fillRect(0, 0, 100, 100)
              ctx.restore()
              ctx.fillRect(0, 0, 100, 100)
              return false
            })
            """)[0]
        guard case .draw(let draw) = list.layers[0] else {
            Issue.record("expected a draw layer")
            return
        }
        let chain = try #require(draw.commands[0].clip).chain
        #expect(chain.count == 2)
        #expect(chain[1].path.boundingBox == CGRect(x: 10, y: 10, width: 10, height: 10))
        #expect(chain[1].rule == .evenOdd)
        #expect(draw.commands[1].clip == nil)
    }

    @Test("drawImage clips the source to the image and scales the destination")
    func drawImageClipping() async throws {
        let list = try await decode("""
            peek.frame(ctx => { ctx.drawImage({ id: 1, width: 10, height: 10 }, -5, 0, 20, 10, 0, 0, 40, 20); return false })
            """)[0]
        guard case .draw(let draw) = list.layers[0], case .image(let id, _, let source, let destination) = draw.commands[0].kind
        else {
            Issue.record("expected an image command")
            return
        }
        #expect(id == 1)
        #expect(source == CGRect(x: 0, y: 0, width: 10, height: 10))
        #expect(destination == CGRect(x: 10, y: 0, width: 20, height: 20))
        #expect(list.paintCount == 0)  // no live image with id 1 in this frame
    }

    @Test("garbage op streams never crash the decoder and are reported once")
    func garbage() {
        let decoder = OpDecoder()
        var generator = SystemRandomNumberGenerator()
        for _ in 0..<300 {
            let count = Int.random(in: 0..<200, using: &generator)
            let ops = (0..<count).map { _ -> Double in
                switch Int.random(in: 0..<6, using: &generator) {
                case 0: Double(OpCode.allCases.randomElement(using: &generator)!.rawValue)
                case 1: Double(Int.random(in: 0..<14, using: &generator))
                case 2: .nan
                case 3: -Double(Int.random(in: 0..<5, using: &generator))
                case 4: .infinity
                default: Double.random(in: -1e6...1e6, using: &generator)
                }
            }
            _ = decoder.decode(ops: ops, strings: ["red", "8px SF Pro"], again: false)
        }
        let list = decoder.decode(ops: [99, 1e9, 1], strings: [], again: false)
        #expect(list.layers.isEmpty)
        let once = decoder.decode(ops: [77, 0, 77, 0], strings: [], again: false)
        #expect(once.diagnostics.isEmpty || once.diagnostics == ["unknown op 77.0 was ignored"])
    }

    @Test("a bad string index or argument count skips only that op")
    func badArguments() {
        let decoder = OpDecoder()
        let ops: [Double] = [
            Double(OpCode.fillColor.rawValue), 1, 5,  // string index out of range
            Double(OpCode.fillRect.rawValue), 3, 0, 0, 10,  // wrong argument count
            Double(OpCode.fillRect.rawValue), 4, 0, 0, 10, 10,
        ]
        let list = decoder.decode(ops: ops, strings: [], again: false)
        guard case .draw(let draw) = list.layers.first else {
            Issue.record("expected a draw layer")
            return
        }
        #expect(draw.commands.count == 1)
        #expect(list.diagnostics.count == 2)
    }

    @Test("canvas arc sweeps: full turns, modulo spans and direction")
    func arcSweep() {
        let tau = 2 * Double.pi
        #expect(CanvasPath.sweep(start: 0, end: 7, counterclockwise: false) == tau)
        #expect(CanvasPath.sweep(start: 0, end: -7, counterclockwise: true) == -tau)
        #expect(abs(CanvasPath.sweep(start: 0, end: -.pi / 2, counterclockwise: false) - 1.5 * .pi) < 1e-12)
        #expect(abs(CanvasPath.sweep(start: 0, end: .pi / 2, counterclockwise: true) + 1.5 * .pi) < 1e-12)
        #expect(CanvasPath.sweep(start: 1, end: 1, counterclockwise: false) == 0)
    }

    @Test("roundRect scales oversized radii and draws a closed subpath")
    func roundRectGeometry() {
        var path = CanvasPath()
        path.apply(.roundRect, [0, 0, 20, 10, 50, 50, 50, 50, 50, 50, 50, 50], transform: .identity)
        #expect(path.path.boundingBox == CGRect(x: 0, y: 0, width: 20, height: 10))
        #expect(path.path.contains(CGPoint(x: 10, y: 5)))
        #expect(!path.path.contains(CGPoint(x: 0.5, y: 0.5)))
    }
}
