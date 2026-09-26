import CoreGraphics
import Foundation

/// The result of one frame on the VM thread.
struct FrameOutcome: Sendable {
    var status: EngineStatus
    var rendered: RenderedFrame?
    var elapsedNanoseconds: UInt64
}

/// Owns one drawing's QuickJS VM, decoder and renderer on a dedicated 4 MiB thread (visual.md B3/B4).
/// Every method only queues work; results come back through completion handlers on the VM thread.
final class DrawingWorker: @unchecked Sendable {
    private struct State {
        var engine: JSEngine?
        var processor = FrameProcessor()
    }

    private let thread: JSThread
    private let state = ThreadConfined(State())
    private let tokenLock = NSLock()
    private var abortToken: AbortToken?
    private let log: @Sendable (String) -> Void

    init(name: String, log: @escaping @Sendable (String) -> Void) {
        thread = JSThread(name: name)
        self.log = log
    }

    /// Creates the VM (prelude included) and runs the script's top-level code once.
    func load(source: [UInt8], filename: String) async -> EngineStatus {
        let log = self.log
        let (status, token): (EngineStatus, AbortToken?) = await thread.run { [state] in
            do {
                let engine = try JSEngine(log: log)
                state.value.engine = engine
                let status = engine.evaluate(source, filename: filename, budget: DrawingLimits.loadBudget)
                return (status, engine.abortToken)
            } catch {
                return (.failed(message: "\(error)"), nil)
            }
        }
        setAbortToken(token)
        return status
    }

    /// Runs `frame(ctx, input)` under the 4 ms deadline, then decodes and renders the frame.
    func frame(input: [UInt8], images: [Int: CGImage], pixels: Int,
               completion: @escaping @Sendable (FrameOutcome) -> Void) {
        let boxedImages = ThreadConfined(images)
        thread.async { [state] in
            guard let engine = state.value.engine else {
                completion(FrameOutcome(status: .failed(message: "the drawing VM is gone"), rendered: nil,
                                        elapsedNanoseconds: 0))
                return
            }
            let frame = engine.frame(input: input, budget: DrawingLimits.frameBudget)
            guard frame.status.isOK else {
                completion(FrameOutcome(status: frame.status, rendered: nil, elapsedNanoseconds: frame.elapsedNanoseconds))
                return
            }
            let rendered = state.value.processor.process(ops: frame.ops, strings: frame.strings, again: frame.again,
                                                         images: boxedImages.value, pixels: pixels)
            completion(FrameOutcome(status: .ok, rendered: rendered, elapsedNanoseconds: frame.elapsedNanoseconds))
        }
    }

    /// Delivers a `peek.on` event under the event deadline.
    func event(name: String, payload: [UInt8]?, completion: @escaping @Sendable (EngineStatus) -> Void) {
        thread.async { [state] in
            guard let engine = state.value.engine else { return }
            completion(engine.event(name: name, payload: payload, budget: DrawingLimits.eventBudget))
        }
    }

    private func setAbortToken(_ token: AbortToken?) {
        tokenLock.lock()
        abortToken = token
        tokenLock.unlock()
    }

    /// Interrupts whatever runs, destroys the VM on its thread and ends the thread.
    func shutdown() {
        tokenLock.lock()
        abortToken?.abort()
        abortToken = nil
        tokenLock.unlock()
        thread.async { [state] in
            state.value.engine = nil
            state.value.processor.reset()
        }
        thread.finish()
    }
}
