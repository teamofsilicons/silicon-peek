import CQuickJS
import Foundation

/// What a VM call ended with (the shim's `PeekStatus`).
enum EngineStatus: Equatable, Sendable {
    case ok
    /// The script threw; `message` and `stack` describe it (stack lines name `file:line:col`).
    case threw(message: String, stack: String?)
    /// The deadline passed (or teardown aborted the call). Scripts cannot catch it.
    case interrupted(message: String)
    /// The 16 MB heap limit was hit: the VM must be destroyed.
    case outOfMemory(message: String)
    /// Internal errors: a missing entry point or malformed input JSON.
    case failed(message: String)

    var isOK: Bool { self == .ok }

    var message: String? {
        switch self {
        case .ok: nil
        case .threw(let message, _), .interrupted(let message), .outOfMemory(let message), .failed(let message):
            message
        }
    }
}

/// One frame as published by `__peek_flush`, copied out of the VM's buffers.
struct EngineFrame: Sendable {
    var status: EngineStatus
    var ops: [Double]
    var strings: [String]
    var again: Bool
    var flushed: Bool
    /// Wall time of the `frame()` call.
    var elapsedNanoseconds: UInt64
    /// CPU time the VM thread spent in the call: the drawing's own cost, unaffected by other load.
    var cpuNanoseconds: UInt64 = 0
}

/// Receives `peek.log` lines on the VM thread.
private final class EngineCallbacks {
    var log: (String) -> Void
    let fonts: FontCache

    init(log: @escaping (String) -> Void, fonts: FontCache) {
        self.log = log
        self.fonts = fonts
    }
}

private let logTrampoline: PeekLogCallback = { opaque, bytes, length in
    guard let opaque, let bytes else { return }
    let callbacks = Unmanaged<EngineCallbacks>.fromOpaque(opaque).takeUnretainedValue()
    callbacks.log(String(decoding: UnsafeRawBufferPointer(start: bytes, count: length), as: UTF8.self))
}

private let measureTrampoline: PeekMeasureCallback = { opaque, font, fontLength, text, textLength, metrics in
    guard let opaque, let metrics else { return }
    let callbacks = Unmanaged<EngineCallbacks>.fromOpaque(opaque).takeUnretainedValue()
    let css = font.map { String(decoding: UnsafeRawBufferPointer(start: $0, count: fontLength), as: UTF8.self) } ?? ""
    let string = text.map { String(decoding: UnsafeRawBufferPointer(start: $0, count: textLength), as: UTF8.self) } ?? ""
    let m = callbacks.fonts.measure(string, css: css)
    metrics[Int(PEEK_TEXT_METRIC_WIDTH.rawValue)] = m.width
    metrics[Int(PEEK_TEXT_METRIC_ACTUAL_LEFT.rawValue)] = m.actualLeft
    metrics[Int(PEEK_TEXT_METRIC_ACTUAL_RIGHT.rawValue)] = m.actualRight
    metrics[Int(PEEK_TEXT_METRIC_ACTUAL_ASCENT.rawValue)] = m.actualAscent
    metrics[Int(PEEK_TEXT_METRIC_ACTUAL_DESCENT.rawValue)] = m.actualDescent
    metrics[Int(PEEK_TEXT_METRIC_FONT_ASCENT.rawValue)] = m.fontAscent
    metrics[Int(PEEK_TEXT_METRIC_FONT_DESCENT.rawValue)] = m.fontDescent
}

/// A QuickJS runtime + context for one drawing, with the prelude loaded (visual.md B3).
///
/// Not thread-safe: create, use and destroy it on one ``JSThread`` (4 MiB stack). Only
/// ``requestAbort()`` may be called from another thread.
final class JSEngine {
    private let vm: OpaquePointer
    private let callbacks: EngineCallbacks
    private let callbacksPointer: UnsafeMutableRawPointer
    /// Lets another thread interrupt this VM safely, even while it is being destroyed.
    let abortToken: AbortToken

    struct CreationError: Error, CustomStringConvertible {
        var description: String
    }

    /// Creates the VM and evaluates the prelude. `log` receives `peek.log` lines and prelude warnings.
    init(memoryLimit: Int = DrawingLimits.memoryLimitBytes, log: @escaping (String) -> Void,
         fonts: FontCache = .shared) throws(CreationError) {
        let callbacks = EngineCallbacks(log: log, fonts: fonts)
        let pointer = Unmanaged.passRetained(callbacks).toOpaque()
        var config = PeekVMConfig()
        config.memory_limit = memoryLimit
        config.max_stack_size = DrawingLimits.maxStackBytes
        config.op_capacity = DrawingLimits.opCapacity
        config.log = logTrampoline
        config.log_opaque = pointer
        config.measure = measureTrampoline
        config.measure_opaque = pointer
        guard let vm = peek_vm_new(&config) else {
            Unmanaged<EngineCallbacks>.fromOpaque(pointer).release()
            throw CreationError(description: "QuickJS could not create a runtime (out of memory)")
        }
        self.vm = vm
        self.callbacks = callbacks
        self.callbacksPointer = pointer
        self.abortToken = AbortToken(vm)

        let prelude = evaluate(Array(Prelude.source.utf8), filename: Prelude.filename, strict: true, budget: .seconds(2))
        guard prelude.isOK else {
            throw CreationError(description: "the drawing prelude failed to load: \(prelude.message ?? "unknown error")")
        }
        let bound = status(peek_vm_bind_entry_points(vm, true))
        guard bound.isOK else {
            throw CreationError(description: "the drawing prelude did not bind its entry points: \(bound.message ?? "")")
        }
    }

    deinit {
        abortToken.invalidate()
        peek_vm_free(vm)
        Unmanaged<EngineCallbacks>.fromOpaque(callbacksPointer).release()
    }

    /// Replaces the log receiver (used when a validator reuses an engine).
    func setLog(_ log: @escaping (String) -> Void) { callbacks.log = log }

    /// Runs a classic script in the global scope (the drawing's top-level code).
    func evaluate(_ source: [UInt8], filename: String, strict: Bool = false, budget: Duration) -> EngineStatus {
        let code = source.withUnsafeBufferPointer { buffer in
            buffer.withMemoryRebound(to: CChar.self) { chars in
                peek_vm_eval(vm, chars.baseAddress, chars.count, filename, strict, Self.deadline(budget))
            }
        }
        return status(code)
    }

    /// Calls `frame(ctx, input)` under the deadline and copies the published frame.
    func frame(input: [UInt8], budget: Duration) -> EngineFrame {
        var output = PeekFrameOutput()
        let started = peek_monotonic_ns()
        let startedCPU = clock_gettime_nsec_np(CLOCK_THREAD_CPUTIME_ID)
        let code = input.withUnsafeBufferPointer { buffer in
            buffer.withMemoryRebound(to: CChar.self) { chars in
                peek_vm_frame(vm, chars.baseAddress, chars.count, Self.deadline(budget), &output)
            }
        }
        let elapsed = peek_monotonic_ns() &- started
        let cpu = clock_gettime_nsec_np(CLOCK_THREAD_CPUTIME_ID) &- startedCPU
        let result = status(code)
        guard result.isOK, output.flushed else {
            return EngineFrame(status: result, ops: [], strings: [], again: false, flushed: false,
                               elapsedNanoseconds: elapsed, cpuNanoseconds: cpu)
        }
        let ops = output.op_count > 0 ? Array(UnsafeBufferPointer(start: output.ops, count: output.op_count)) : []
        var strings: [String] = []
        strings.reserveCapacity(output.string_count)
        if output.string_count > 0, let bytes = output.string_bytes, let offsets = output.string_offsets,
           let lengths = output.string_lengths {
            let base = UnsafeRawPointer(bytes)
            for index in 0..<output.string_count {
                let start = base.advanced(by: Int(offsets[index]))
                strings.append(String(decoding: UnsafeRawBufferPointer(start: start, count: Int(lengths[index])),
                                      as: UTF8.self))
            }
        }
        return EngineFrame(status: result, ops: ops, strings: strings, again: output.again, flushed: true,
                           elapsedNanoseconds: elapsed, cpuNanoseconds: cpu)
    }

    /// Calls the drawing's `peek.on(name)` handlers with a JSON payload (`nil` → `undefined`).
    func event(name: String, payload: [UInt8]?, budget: Duration) -> EngineStatus {
        let code: PeekStatus
        if let payload {
            code = payload.withUnsafeBufferPointer { buffer in
                buffer.withMemoryRebound(to: CChar.self) { chars in
                    peek_vm_event(vm, name, chars.baseAddress, chars.count, Self.deadline(budget))
                }
            }
        } else {
            code = peek_vm_event(vm, name, nil, 0, Self.deadline(budget))
        }
        return status(code)
    }

    var memoryUsed: Int { peek_vm_memory_used(vm) }

    func collectGarbage() { peek_vm_run_gc(vm) }


    private func status(_ code: PeekStatus) -> EngineStatus {
        switch code {
        case PEEK_STATUS_OK:
            return .ok
        case PEEK_STATUS_THREW:
            let stack = errorStack
            return .threw(message: errorMessage, stack: stack.isEmpty ? nil : stack)
        case PEEK_STATUS_INTERRUPTED:
            return .interrupted(message: errorMessage)
        case PEEK_STATUS_OUT_OF_MEMORY:
            return .outOfMemory(message: errorMessage)
        default:
            return .failed(message: errorMessage)
        }
    }

    private var errorMessage: String {
        var length = 0
        guard let pointer = peek_vm_error_message(vm, &length) else { return "" }
        return String(decoding: UnsafeRawBufferPointer(start: pointer, count: length), as: UTF8.self)
    }

    private var errorStack: String {
        var length = 0
        guard let pointer = peek_vm_error_stack(vm, &length) else { return "" }
        return String(decoding: UnsafeRawBufferPointer(start: pointer, count: length), as: UTF8.self)
    }

    static func deadline(_ budget: Duration) -> UInt64 {
        let (seconds, attoseconds) = budget.components
        let nanos = UInt64(max(0, seconds)) &* 1_000_000_000 &+ UInt64(max(0, attoseconds) / 1_000_000_000)
        return peek_monotonic_ns() &+ max(1, nanos)
    }
}

/// Interrupts a VM from any thread (teardown, unload). Safe after the VM is gone: it then does nothing.
final class AbortToken: @unchecked Sendable {
    private let lock = NSLock()
    private var vm: OpaquePointer?

    fileprivate init(_ vm: OpaquePointer) { self.vm = vm }

    /// Makes the running call and every later one return `interrupted`.
    func abort() {
        lock.lock()
        defer { lock.unlock() }
        if let vm { peek_vm_request_abort(vm) }
    }

    fileprivate func invalidate() {
        lock.lock()
        vm = nil
        lock.unlock()
    }
}
