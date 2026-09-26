import Darwin
import Foundation
import PeekCore

struct FakeServerError: Error, CustomStringConvertible {
    let description: String
    init(_ description: String) { self.description = description }
}

/// An in-process stand-in for peekd: a real AF_UNIX listener in a private temp
/// directory (mode 0700 by default), accepting connections on a background thread.
final class FakePeekd: @unchecked Sendable {
    let directory: String
    let socketPath: String
    private let listenFD: Int32
    private let lock = NSLock()
    private var stopped = false
    private var queue: [FakeConnection] = []
    private var all: [FakeConnection] = []

    init(directoryMode: mode_t = 0o700) throws {
        var template = Array((NSTemporaryDirectory() + "pk.XXXXXX").utf8CString)
        guard let created = mkdtemp(&template) else { throw FakeServerError("mkdtemp failed: errno \(errno)") }
        directory = String(cString: created)
        chmod(directory, directoryMode)
        socketPath = directory + "/d.sock"

        listenFD = socket(AF_UNIX, SOCK_STREAM, 0)
        guard listenFD >= 0 else { throw FakeServerError("socket failed: errno \(errno)") }
        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(socketPath.utf8)
        guard bytes.count < 104 else { throw FakeServerError("temp socket path too long: \(socketPath)") }
        withUnsafeMutableBytes(of: &address.sun_path) { raw in
            raw.copyBytes(from: bytes)
            raw[bytes.count] = 0
        }
        let fd = listenFD
        let bound = withUnsafePointer(to: &address) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard bound == 0, listen(listenFD, 8) == 0 else { throw FakeServerError("bind/listen failed: errno \(errno)") }
        chmod(socketPath, 0o600)

        let thread = Thread { [self] in acceptLoop() }
        thread.name = "FakePeekd.accept"
        thread.start()
    }

    private func acceptLoop() {
        while !lock.withLock({ stopped }) {
            var descriptor = pollfd(fd: listenFD, events: Int16(POLLIN), revents: 0)
            guard poll(&descriptor, 1, 20) > 0 else { continue }
            let fd = accept(listenFD, nil, nil)
            guard fd >= 0 else { continue }
            let connection = FakeConnection(fd: fd)
            lock.withLock {
                queue.append(connection)
                all.append(connection)
            }
        }
    }

    /// Number of connections accepted so far.
    var acceptedCount: Int { lock.withLock { all.count } }

    /// The next accepted connection, or throws after `timeout`.
    func nextConnection(timeout: Duration = .seconds(5)) async throws -> FakeConnection {
        let deadline = ContinuousClock.now + timeout
        while ContinuousClock.now < deadline {
            if let connection = lock.withLock({ queue.isEmpty ? nil : queue.removeFirst() }) { return connection }
            try await Task.sleep(for: .milliseconds(5))
        }
        throw FakeServerError("no client connected within \(timeout)")
    }

    /// Accepts a connection, checks its hello and answers with `result`.
    func acceptHandshake(result: JSONValue = ["protocol": 1, "peekd_version": "0.1.0-test"]) async throws -> FakeConnection {
        let connection = try await nextConnection()
        let hello = try await connection.nextFrame()
        guard hello["op"] == "hello", let id = hello["id"]?.stringValue else {
            throw FakeServerError("first frame was not hello: \(hello)")
        }
        try connection.write(FrameCoding.okReply(id: id, result: result))
        return connection
    }

    func shutdown() {
        let connections = lock.withLock {
            stopped = true
            return all
        }
        for connection in connections { connection.close() }
        close(listenFD)
        unlink(socketPath)
        rmdir(directory)
    }
}

/// The server side of one connection. Reads block for at most 5 s.
final class FakeConnection: @unchecked Sendable {
    let fd: Int32
    private let readLock = NSLock()
    private let stateLock = NSLock()
    private var decoder = FrameDecoder()
    private var closed = false

    init(fd: Int32) {
        self.fd = fd
        var timeout = timeval(tv_sec: 5, tv_usec: 0)
        setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, socklen_t(MemoryLayout<timeval>.size))
        var one: Int32 = 1
        setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &one, socklen_t(MemoryLayout<Int32>.size))
    }

    /// Blocks until a full frame arrives; throws on EOF, error or timeout.
    func readFrame() throws -> Frame {
        try readLock.withLock {
            var buffer = [UInt8](repeating: 0, count: 65536)
            while true {
                if let frame = try decoder.next() { return frame }
                let count = recv(fd, &buffer, buffer.count, 0)
                if count == 0 { throw FakeServerError("client closed the connection") }
                if count < 0 { throw FakeServerError("recv failed: errno \(errno)") }
                decoder.append(Data(buffer[0..<count]))
            }
        }
    }

    func nextFrame() async throws -> Frame {
        try await withCheckedThrowingContinuation { continuation in
            Thread.detachNewThread { [self] in
                do {
                    continuation.resume(returning: try readFrame())
                } catch {
                    continuation.resume(throwing: error)
                }
            }
        }
    }

    /// True when the client closed its end (the next read hits EOF).
    func waitForClientClose() async -> Bool {
        do {
            _ = try await nextFrame()
            return false
        } catch {
            return "\(error)".contains("closed")
        }
    }

    func write(_ frame: Frame) throws { try writeRaw(frame.encoded()) }

    func writeRaw(_ data: Data) throws {
        try data.withUnsafeBytes { raw in
            var offset = 0
            while offset < raw.count {
                let n = send(fd, raw.baseAddress!.advanced(by: offset), raw.count - offset, 0)
                if n < 0 { throw FakeServerError("send failed: errno \(errno)") }
                offset += n
            }
        }
    }

    func close() {
        stateLock.withLock {
            guard !closed else { return }
            closed = true
            shutdown(fd, SHUT_RDWR)
        }
    }

    deinit { Darwin.close(fd) }
}
