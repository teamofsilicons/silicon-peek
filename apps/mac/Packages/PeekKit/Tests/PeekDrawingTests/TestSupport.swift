import CoreGraphics
import Foundation
import PeekCore
import Testing

@testable import PeekDrawing

/// Runs a JSEngine (and its decoder) on a 4 MiB JSThread, like the real host.
final class EngineHarness: @unchecked Sendable {
    let thread = JSThread(name: "test.drawing")
    private let box = ThreadConfined<(engine: JSEngine?, decoder: OpDecoder, logs: [String])>((nil, OpDecoder(), []))

    init() async throws {
        let created: String? = await thread.run { [box] () -> String? in
            do {
                box.value.engine = try JSEngine(log: { line in box.value.logs.append(line) })
                return nil
            } catch {
                return "\(error)"
            }
        }
        if let created { throw HarnessError(created) }
    }

    deinit { thread.finish() }

    var logs: [String] {
        get async { await thread.run { [box] in box.value.logs } }
    }

    func load(_ source: String, filename: String = "drawing.js") async -> EngineStatus {
        await thread.run { [box] in
            box.value.engine!.evaluate(Array(source.utf8), filename: filename, budget: .milliseconds(500))
        }
    }

    func frame(_ input: InputSnapshot = .sample(), budget: Duration = .seconds(2)) async -> EngineFrame {
        let bytes = input.jsonBytes()
        return await thread.run { [box] in box.value.engine!.frame(input: bytes, budget: budget) }
    }

    func decode(_ input: InputSnapshot = .sample(), dump: Bool = false, images: [Int: CGImage] = [:]) async
        -> (EngineFrame, DisplayList?)
    {
        let bytes = input.jsonBytes()
        return await thread.run { [box] in
            let frame = box.value.engine!.frame(input: bytes, budget: .seconds(2))
            guard frame.status.isOK else { return (frame, nil) }
            let list = box.value.decoder.decode(ops: frame.ops, strings: frame.strings, again: frame.again,
                                                images: images, recordDump: dump)
            return (frame, list)
        }
    }

    func event(_ event: DrawingEvent) async -> EngineStatus {
        let name = event.name, payload = event.payloadJSON
        return await thread.run { [box] in
            box.value.engine!.event(name: name, payload: payload, budget: .seconds(2))
        }
    }
}

struct HarnessError: Error, CustomStringConvertible {
    let description: String
    init(_ description: String) { self.description = description }
}

extension InputSnapshot {
    static func sample(t: Double = 0, dt: Double = 0, phase: Phase = .showing, mode: DisplayMode = .normal)
        -> InputSnapshot
    {
        InputSnapshot(t: t, dt: dt, slot: .init(index: .bottom, facing: -.pi / 2), mode: mode, phase: phase)
    }
}

/// Decoded op names of a frame, e.g. ["fillStyle", "beginPath", …] (from the dump representation).
func opNames(_ list: DisplayList) -> [String] {
    list.layers.flatMap { layer -> [String] in
        guard case .draw(let draw) = layer else { return ["<\(layer.kindName)>"] }
        return (draw.dumpOps ?? []).compactMap { $0.arrayValue?.first?.stringValue }
    }
}

/// Reads one pixel of an image (row 0 = top) as sRGB 0…255 RGBA, un-premultiplied.
func pixel(_ image: CGImage, x: Int, y: Int) -> (r: Int, g: Int, b: Int, a: Int) {
    let space = CGColorSpace(name: CGColorSpace.sRGB)!
    var data = [UInt8](repeating: 0, count: 4)
    let context = CGContext(data: &data, width: 1, height: 1, bitsPerComponent: 8, bytesPerRow: 4, space: space,
                            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    context.interpolationQuality = .none
    context.draw(image, in: CGRect(x: -x, y: -(image.height - 1 - y), width: image.width, height: image.height))
    let a = Int(data[3])
    guard a > 0 else { return (0, 0, 0, 0) }
    func un(_ c: UInt8) -> Int { min(255, Int((Double(c) * 255 / Double(a)).rounded())) }
    return (un(data[0]), un(data[1]), un(data[2]), a)
}

/// Polls `condition` on the main actor until it holds or `timeout` passes.
@MainActor
func waitUntil(timeout: Duration = .seconds(5), _ condition: () -> Bool) async -> Bool {
    let clock = ContinuousClock()
    let deadline = clock.now + timeout
    while !condition() {
        if clock.now > deadline { return false }
        try? await Task.sleep(for: .milliseconds(2))
    }
    return true
}

/// Images keyed by handle id.
@MainActor
final class FakeImages: ImageProviding {
    var images: [Int: CGImage] = [:]
    func prepare(sendID: String, paths: [String]) async -> [String: PreparedImage] { [:] }
    func image(for handle: ImageHandle) -> CGImage? { images[handle.id] }
    func release(sendID: String) {}
}

/// An input source that records the t/dt it was asked for.
@MainActor
final class FakeInputSource: DrawingInputSource {
    var onWake: (@MainActor (WakeReason) -> Void)?
    var base = InputSnapshot.sample()
    private(set) var requests: [(t: Double, dt: Double)] = []

    func snapshot(t: Double, dt: Double) -> InputSnapshot {
        requests.append((t, dt))
        var snapshot = base
        snapshot.t = t
        snapshot.dt = dt
        return snapshot
    }
}

/// A solid-colour test image.
func solidImage(width: Int, height: Int, red: Double, green: Double, blue: Double) -> CGImage {
    let context = CGContext(data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
                            space: CGColorSpace(name: CGColorSpace.sRGB)!,
                            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    context.setFillColor(CGColor(srgbRed: red, green: green, blue: blue, alpha: 1))
    context.fill(CGRect(x: 0, y: 0, width: width, height: height))
    return context.makeImage()!
}

/// Decodes and renders one script frame to a 100 × 100 px sRGB image (1 px per unit).
func renderScript(_ body: String, images: [Int: CGImage] = [:], input: InputSnapshot = .sample(),
                  pixels: Int = 100) async throws -> CGImage {
    let harness = try await EngineHarness()
    let status = await harness.load("peek.frame((ctx, input) => { \(body)\n return false })")
    guard status == .ok else { throw HarnessError("load: \(status)") }
    let (frame, list) = await harness.decode(input, images: images)
    guard let list else { throw HarnessError("frame: \(frame.status)") }
    let context = CGReplay.makeContext(pixels: pixels, colorSpace: CGReplay.sRGB)!
    for layer in list.layers {
        switch layer {
        case .draw(let draw):
            CGReplay.replay(draw.commands, in: context, pixels: pixels,
                            environment: ReplayEnvironment(images: images, monochrome: draw.vibrant))
        case .glass(let glass):
            CGReplay.drawFlatGlass(glass.unitPath, rule: glass.rule, tint: glass.tint, in: context)
        case .blur(let blur):
            CGReplay.drawFlatBlur(blur.path, rule: blur.rule, in: context)
        }
    }
    return context.makeImage()!
}
