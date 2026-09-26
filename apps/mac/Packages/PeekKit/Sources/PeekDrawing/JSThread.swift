import Foundation

/// A serial executor backed by one dedicated `Thread` with a 4 MiB stack (BLUEPRINT §8.6).
///
/// QuickJS's stack limit is 1 MiB while a default secondary thread has 512 KiB, so every VM call must run
/// here. Jobs run in submission order; `finish()` lets the queue drain and ends the thread.
final class JSThread: @unchecked Sendable {
    private let condition = NSCondition()
    private var jobs: [@Sendable () -> Void] = []
    private var finishing = false

    init(name: String, stackSize: Int = DrawingLimits.threadStackBytes) {
        let thread = Thread { [self] in self.run() }
        thread.name = name
        thread.stackSize = stackSize
        thread.qualityOfService = .userInteractive
        thread.start()
    }

    /// Queues a job. Jobs submitted after `finish()` are dropped.
    func async(_ job: @escaping @Sendable () -> Void) {
        condition.lock()
        defer { condition.unlock() }
        guard !finishing else { return }
        jobs.append(job)
        condition.signal()
    }

    /// Runs `body` on the thread and returns its result.
    func run<T: Sendable>(_ body: @escaping @Sendable () -> T) async -> T {
        await withCheckedContinuation { (continuation: CheckedContinuation<T, Never>) in
            let accepted = enqueueIfRunning { continuation.resume(returning: body()) }
            if !accepted {
                // The thread is finishing: run inline rather than leak the continuation. Callers only
                // do this for teardown work that is safe on any thread.
                continuation.resume(returning: body())
            }
        }
    }

    /// Lets queued jobs finish, then ends the thread.
    func finish() {
        condition.lock()
        finishing = true
        condition.signal()
        condition.unlock()
    }

    private func enqueueIfRunning(_ job: @escaping @Sendable () -> Void) -> Bool {
        condition.lock()
        defer { condition.unlock() }
        guard !finishing else { return false }
        jobs.append(job)
        condition.signal()
        return true
    }

    private func run() {
        while true {
            condition.lock()
            while jobs.isEmpty && !finishing {
                condition.wait()
            }
            if jobs.isEmpty && finishing {
                condition.unlock()
                break
            }
            let batch = jobs
            jobs.removeAll(keepingCapacity: true)
            condition.unlock()
            for job in batch {
                autoreleasepool { job() }
            }
        }
    }
}

/// A value that is only touched on one ``JSThread`` (the VM, the decoder and the renderer). Swift cannot
/// see that confinement, so the box is `@unchecked Sendable`; every access happens inside a job.
final class ThreadConfined<Value>: @unchecked Sendable {
    var value: Value
    init(_ value: Value) { self.value = value }
}
