import CQuickJS
import Foundation
import Testing

/// A minimal prelude that honours the shim contract in peek_qjs.h. The real
/// prelude (canvas state, op encoding) belongs to PeekDrawing.
private let testPrelude = #"""
(function () {
  const ops = __peek_ops, flush = __peek_flush, log = __peek_log;
  let frameFn = null;
  const handlers = Object.create(null);
  globalThis.peek = Object.freeze({
    frame(fn) { frameFn = fn; },
    on(name, fn) { handlers[name] = fn; },
    log(...args) { log(args.map(String).join(' ')); },
  });
  globalThis.__peek_frame = function (input) {
    let n = 0;
    const strings = [];
    const ctx = {
      op(code, ...args) { ops[n++] = code; ops[n++] = args.length; for (const a of args) ops[n++] = a; },
      str(s) { strings.push(s); return strings.length - 1; },
    };
    const again = frameFn ? frameFn(ctx, input) : false;
    flush(n, strings, again === true);
  };
  globalThis.__peek_event = function (name, payload) {
    const h = handlers[name];
    if (h) h(payload);
  };
})();
"""#

private final class LogSink: @unchecked Sendable {
    var lines: [String] = []
}

private let logCallback: PeekLogCallback = { opaque, bytes, length in
    guard let opaque, let bytes else { return }
    let sink = Unmanaged<LogSink>.fromOpaque(opaque).takeUnretainedValue()
    let buffer = UnsafeRawBufferPointer(start: bytes, count: length)
    sink.lines.append(String(decoding: buffer, as: UTF8.self))
}

/// Owns one VM for a test. Test threads have 512 KiB stacks, so the JS stack is capped at 256 KiB.
private final class TestVM {
    let vm: OpaquePointer
    let sink = LogSink()

    init(memoryLimit: Int = 0, opCapacity: Int = 0, prelude: Bool = true, hideGlobals: Bool = true,
         measure: PeekMeasureCallback? = nil) throws {
        var config = PeekVMConfig()
        config.memory_limit = memoryLimit
        config.max_stack_size = 256 << 10
        config.op_capacity = opCapacity
        config.log = logCallback
        config.log_opaque = Unmanaged.passUnretained(sink).toOpaque()
        config.measure = measure
        guard let vm = peek_vm_new(&config) else { throw ShimFailure("peek_vm_new returned NULL") }
        self.vm = vm
        if prelude {
            try expectOK(eval(testPrelude, filename: "prelude.js", strict: true))
            try expectOK(peek_vm_bind_entry_points(vm, hideGlobals))
        }
    }

    deinit { peek_vm_free(vm) }

    func eval(_ source: String, filename: String = "drawing.js", strict: Bool = false,
              budget: Duration? = .seconds(2)) -> PeekStatus {
        let bytes = Array(source.utf8)
        return bytes.withUnsafeBufferPointer { buffer in
            buffer.baseAddress!.withMemoryRebound(to: CChar.self, capacity: bytes.count) { pointer in
                peek_vm_eval(vm, pointer, bytes.count, filename, strict, deadline(budget))
            }
        }
    }

    func frame(_ json: String, budget: Duration? = .seconds(2)) -> (PeekStatus, PeekFrameOutput) {
        var output = PeekFrameOutput()
        let bytes = Array(json.utf8)
        let status = bytes.withUnsafeBufferPointer { buffer in
            buffer.baseAddress!.withMemoryRebound(to: CChar.self, capacity: bytes.count) { pointer in
                peek_vm_frame(vm, pointer, bytes.count, deadline(budget), &output)
            }
        }
        return (status, output)
    }

    func event(_ name: String, _ json: String?) -> PeekStatus {
        guard let json else { return peek_vm_event(vm, name, nil, 0, deadline(.seconds(2))) }
        let bytes = Array(json.utf8)
        return bytes.withUnsafeBufferPointer { buffer in
            buffer.baseAddress!.withMemoryRebound(to: CChar.self, capacity: bytes.count) { pointer in
                peek_vm_event(vm, name, pointer, bytes.count, deadline(.seconds(2)))
            }
        }
    }

    var errorMessage: String {
        var length = 0
        let pointer = peek_vm_error_message(vm, &length)!
        return String(decoding: UnsafeRawBufferPointer(start: pointer, count: length), as: UTF8.self)
    }

    var errorStack: String {
        var length = 0
        let pointer = peek_vm_error_stack(vm, &length)!
        return String(decoding: UnsafeRawBufferPointer(start: pointer, count: length), as: UTF8.self)
    }

    private func deadline(_ budget: Duration?) -> UInt64 {
        guard let budget else { return 0 }
        let (seconds, attoseconds) = budget.components
        return peek_monotonic_ns() + UInt64(seconds) * 1_000_000_000 + UInt64(attoseconds / 1_000_000_000)
    }

    private func expectOK(_ status: PeekStatus) throws {
        guard status == PEEK_STATUS_OK else {
            throw ShimFailure("status \(status.rawValue): \(errorMessage)\n\(errorStack)")
        }
    }
}

private struct ShimFailure: Error, CustomStringConvertible {
    let description: String
    init(_ description: String) { self.description = description }
}

private func strings(of output: PeekFrameOutput) -> [String] {
    (0..<output.string_count).map { index in
        let offset = Int(output.string_offsets[index])
        let length = Int(output.string_lengths[index])
        let start = UnsafeRawPointer(output.string_bytes!).advanced(by: offset)
        return String(decoding: UnsafeRawBufferPointer(start: start, count: length), as: UTF8.self)
    }
}

private func ops(of output: PeekFrameOutput) -> [Double] {
    Array(UnsafeBufferPointer(start: output.ops, count: output.op_count))
}

/// A fake measurer: width = 10 × UTF-8 bytes of the text, font ascent = bytes of the font string, the rest fixed.
private let fakeMeasure: PeekMeasureCallback = { _, _, fontLength, _, textLength, metrics in
    guard let metrics else { return }
    metrics[Int(PEEK_TEXT_METRIC_WIDTH.rawValue)] = Double(textLength) * 10
    metrics[Int(PEEK_TEXT_METRIC_ACTUAL_LEFT.rawValue)] = 1
    metrics[Int(PEEK_TEXT_METRIC_ACTUAL_RIGHT.rawValue)] = 2
    metrics[Int(PEEK_TEXT_METRIC_ACTUAL_ASCENT.rawValue)] = 3
    metrics[Int(PEEK_TEXT_METRIC_ACTUAL_DESCENT.rawValue)] = 4
    metrics[Int(PEEK_TEXT_METRIC_FONT_ASCENT.rawValue)] = Double(fontLength)
    metrics[Int(PEEK_TEXT_METRIC_FONT_DESCENT.rawValue)] = 6
}

@Suite("peek_qjs shim")
struct PeekQJSTests {
    @Test("__peek_measure returns the measure callback's 7 metrics (zeros without a callback)")
    func measure() throws {
        let script = #"__peek_log(JSON.stringify(__peek_measure("12px system-ui", "héllo")))"#
        let measured = try TestVM(prelude: false, measure: fakeMeasure)
        #expect(measured.eval(script) == PEEK_STATUS_OK, "\(measured.errorMessage)")
        // "héllo" is 6 UTF-8 bytes; "12px system-ui" is 14.
        #expect(measured.sink.lines == ["[60,1,2,3,4,14,6]"])
        let plain = try TestVM(prelude: false)
        #expect(plain.eval(script) == PEEK_STATUS_OK)
        #expect(plain.sink.lines == ["[0,0,0,0,0,0,0]"])
    }

    @Test("reports the vendored quickjs-ng version")
    func version() {
        #expect(String(cString: peek_qjs_version()) == "0.17.0")
    }

    @Test("a frame publishes ops, strings and the again flag through __peek_flush")
    func frameFlush() throws {
        let vm = try TestVM()
        #expect(vm.eval("""
            peek.frame((ctx, input) => {
              ctx.op(1, input.t, 2)
              ctx.op(2, ctx.str('hello'), ctx.str('wörld ✓'))
              return input.t < 1
            })
            """) == PEEK_STATUS_OK)

        let (status, output) = vm.frame(#"{"t":0.5}"#)
        #expect(status == PEEK_STATUS_OK)
        #expect(output.flushed)
        #expect(output.again)
        #expect(ops(of: output) == [1, 2, 0.5, 2, 2, 2, 0, 1])
        #expect(strings(of: output) == ["hello", "wörld ✓"])

        let (status2, output2) = vm.frame(#"{"t":3}"#)
        #expect(status2 == PEEK_STATUS_OK)
        #expect(!output2.again)
        #expect(ops(of: output2) == [1, 2, 3, 2, 2, 2, 0, 1])
    }

    @Test("an uncaught exception reports THREW with the message and a stack naming the file")
    func threw() throws {
        let vm = try TestVM()
        #expect(vm.eval("peek.frame((ctx, input) => input.show.elements.length)", filename: "cassette.js")
            == PEEK_STATUS_OK)
        let (status, output) = vm.frame(#"{"show":null}"#)
        #expect(status == PEEK_STATUS_THREW)
        #expect(!output.flushed)
        #expect(vm.errorMessage.hasPrefix("TypeError"))
        #expect(vm.errorStack.contains("cassette.js"))
    }

    @Test("an endless loop is interrupted at the deadline and try/catch cannot swallow it")
    func deadline() throws {
        let vm = try TestVM()
        #expect(vm.eval("peek.frame(() => { try { for (;;) {} } catch (e) { return true } })") == PEEK_STATUS_OK)
        let clock = ContinuousClock()
        let started = clock.now
        let (status, output) = vm.frame("{}", budget: .milliseconds(20))
        let elapsed = clock.now - started
        #expect(status == PEEK_STATUS_INTERRUPTED)
        #expect(!output.flushed)
        #expect(vm.errorMessage.contains("interrupted"))
        #expect(elapsed < .seconds(1))

        // The context stays usable after an interrupt.
        #expect(vm.eval("globalThis.after = 1 + 1") == PEEK_STATUS_OK)
    }

    @Test("an abort request interrupts every call until it is cleared")
    func abort() throws {
        let vm = try TestVM()
        #expect(vm.eval("peek.frame(() => { let s = 0; for (let i = 0; i < 1e6; i++) s += i; return false })")
            == PEEK_STATUS_OK)
        peek_vm_request_abort(vm.vm)
        #expect(vm.frame("{}", budget: nil).0 == PEEK_STATUS_INTERRUPTED)
        peek_vm_clear_abort(vm.vm)
        #expect(vm.frame("{}", budget: nil).0 == PEEK_STATUS_OK)
    }

    @Test("exceeding the memory limit reports OUT_OF_MEMORY")
    func outOfMemory() throws {
        let vm = try TestVM(memoryLimit: 8 << 20)
        #expect(vm.eval("peek.frame(() => { const keep = []; for (;;) keep.push(new Array(4096).fill(1.5)) })")
            == PEEK_STATUS_OK)
        let (status, _) = vm.frame("{}", budget: .seconds(10))
        #expect(status == PEEK_STATUS_OUT_OF_MEMORY)
        #expect(vm.errorMessage.contains("out of memory"))
    }

    @Test("events reach __peek_event and peek.log reaches the log callback")
    func eventsAndLog() throws {
        let vm = try TestVM()
        #expect(vm.eval("""
            peek.on('click', e => peek.log('click', e.x, e.y, e.count))
            peek.on('enter', e => peek.log('enter', typeof e))
            """) == PEEK_STATUS_OK)
        #expect(vm.event("click", #"{"x":12.5,"y":40,"count":2}"#) == PEEK_STATUS_OK)
        #expect(vm.event("enter", nil) == PEEK_STATUS_OK)
        #expect(vm.event("unknown", "{}") == PEEK_STATUS_OK)
        #expect(vm.sink.lines == ["click 12.5 40 2", "enter undefined"])
    }

    @Test("drawings get no std/os modules, timers, network or module loader")
    func sandbox() throws {
        let vm = try TestVM()
        let status = vm.eval("""
            for (const name of ['std', 'os', 'setTimeout', 'setInterval', 'fetch', 'XMLHttpRequest',
                                'require', 'WebAssembly', 'Date', '__peek_ops', '__peek_flush', '__peek_log',
                                '__peek_frame', '__peek_event']) {
              if (typeof globalThis[name] !== 'undefined') throw new Error(name + ' is reachable')
            }
            if (typeof JSON.parse !== 'function' || typeof Map !== 'function' || typeof Float64Array !== 'function'
                || typeof Promise !== 'function' || !/a+/.test('caa') || Math.random() >= 1) throw new Error('missing built-in')
            """)
        #expect(status == PEEK_STATUS_OK, "\(vm.errorMessage)")
    }

    @Test("promise jobs run after the frame")
    func promiseJobs() throws {
        let vm = try TestVM()
        #expect(vm.eval("""
            let resolved = 0
            peek.frame((ctx) => { Promise.resolve().then(() => { resolved++ }); ctx.op(9, resolved); return false })
            """) == PEEK_STATUS_OK)
        #expect(ops(of: vm.frame("{}").1) == [9, 1, 0])
        #expect(ops(of: vm.frame("{}").1) == [9, 1, 1])
    }

    @Test("calls fail with MISSING_ENTRY until __peek_frame is bound")
    func missingEntry() throws {
        let vm = try TestVM(prelude: false)
        #expect(vm.frame("{}").0 == PEEK_STATUS_MISSING_ENTRY)
        #expect(vm.event("enter", nil) == PEEK_STATUS_MISSING_ENTRY)
        #expect(peek_vm_bind_entry_points(vm.vm, true) == PEEK_STATUS_MISSING_ENTRY)
        #expect(vm.errorMessage.contains("__peek_frame"))
    }

    @Test("invalid input JSON is refused as INVALID_ARGUMENT")
    func invalidInput() throws {
        let vm = try TestVM()
        #expect(vm.eval("peek.frame(() => false)") == PEEK_STATUS_OK)
        #expect(vm.frame("{not json").0 == PEEK_STATUS_INVALID_ARGUMENT)
        #expect(vm.frame("{}").0 == PEEK_STATUS_OK)
    }

    @Test("__peek_flush rejects an op count beyond the buffer capacity")
    func flushBounds() throws {
        let vm = try TestVM(opCapacity: 16, hideGlobals: false)
        #expect(peek_vm_op_capacity(vm.vm) == 16)
        #expect(vm.eval("globalThis.__peek_frame = () => __peek_flush(17, [], false)") == PEEK_STATUS_OK)
        #expect(peek_vm_bind_entry_points(vm.vm, false) == PEEK_STATUS_OK)
        let (status, output) = vm.frame("{}")
        #expect(status == PEEK_STATUS_THREW)
        #expect(!output.flushed)
        #expect(vm.errorMessage.hasPrefix("RangeError"))
    }

    @Test("strict evaluation rejects sloppy-mode assignments")
    func strictMode() throws {
        let vm = try TestVM()
        #expect(vm.eval("undeclaredGlobal = 1", strict: true) == PEEK_STATUS_THREW)
        #expect(vm.errorMessage.hasPrefix("ReferenceError"))
        #expect(vm.eval("undeclaredGlobal = 1", strict: false) == PEEK_STATUS_OK)
    }

    @Test("memory usage is reported and garbage collection runs")
    func memory() throws {
        let vm = try TestVM()
        let before = peek_vm_memory_used(vm.vm)
        #expect(before > 0)
        peek_vm_run_gc(vm.vm)
        #expect(peek_vm_memory_used(vm.vm) > 0)
    }
}
