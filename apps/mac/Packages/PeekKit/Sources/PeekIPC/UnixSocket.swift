import Darwin
import Foundation
import PeekCore

/// Where peekd listens (BLUEPRINT §1.6, D4).
public enum DaemonSocket {
    /// Test-only override of the socket path.
    public static let environmentOverride = "PEEK_DAEMON_SOCKET"
    /// `sockaddr_un.sun_path` is 104 bytes on macOS, including the terminating NUL.
    public static let sunPathCapacity = 104

    /// `/var/tmp/silicon-peek-<uid>/peekd.sock` (36 bytes for uid 501).
    public static func defaultPath(uid: uid_t = getuid()) -> String {
        "/var/tmp/silicon-peek-\(uid)/peekd.sock"
    }

    /// The override when set and non-empty, else the default path.
    public static func resolvePath(environment: [String: String] = ProcessInfo.processInfo.environment,
                                   uid: uid_t = getuid()) -> String {
        if let override = environment[environmentOverride], !override.isEmpty { return override }
        return defaultPath(uid: uid)
    }
}

/// Why the socket to peekd could not be opened. Messages say what failed, why, and what to do.
public enum SocketConnectError: Error, Sendable, Equatable, CustomStringConvertible {
    case pathTooLong(path: String, bytes: Int)
    case directoryMissing(path: String)
    case directoryInsecure(path: String, reason: String)
    case socketMissing(path: String)
    case notASocket(path: String)
    case socketOwnedByOtherUser(path: String, owner: uid_t)
    case refused(path: String)
    case peerIdentityMismatch(path: String, expected: uid_t, actual: uid_t)
    case systemCall(name: String, path: String, errno: Int32)

    public var description: String {
        switch self {
        case .pathTooLong(let path, let bytes):
            return "socket path \(path) is \(bytes) bytes; macOS allows at most \(DaemonSocket.sunPathCapacity - 1). Use a shorter \(DaemonSocket.environmentOverride)."
        case .directoryMissing(let path):
            return "peekd is not running: \(path) does not exist. Start it with `open ~/Applications/Peek.app` (the app registers peekd) or `peek daemon restart`."
        case .directoryInsecure(let path, let reason):
            return "refusing to connect through \(path): \(reason). Remove the directory and restart peekd so it is recreated with mode 0700."
        case .socketMissing(let path):
            return "peekd is not running: no socket at \(path). Start it with `peek daemon restart`, or check ~/Library/Application Support/Peek/peekd.log."
        case .notASocket(let path):
            return "\(path) exists but is not a Unix socket. Remove it and restart peekd."
        case .socketOwnedByOtherUser(let path, let owner):
            return "\(path) belongs to uid \(owner), not to this user (uid \(getuid())). Refusing to talk to another user's daemon."
        case .refused(let path):
            return "peekd refused the connection at \(path) (stale socket or peekd is restarting); retrying."
        case .peerIdentityMismatch(let path, let expected, let actual):
            return "the process listening on \(path) runs as uid \(actual), not uid \(expected). Refusing to talk to it."
        case .systemCall(let name, let path, let code):
            return "\(name)() failed for \(path): \(String(cString: strerror(code))) (errno \(code))."
        }
    }
}

/// A connected, peer-verified AF_UNIX stream socket.
///
/// Thread model: exactly one reader thread calls ``receive(into:)`` and, when it is
/// done, ``closeAfterReading()``. Any thread may call ``send(_:)`` and ``shutdown()``.
/// The descriptor is only closed by the reader, after `shutdown` woke it, so a
/// writer can never hit a recycled descriptor.
final class UnixSocket: @unchecked Sendable {
    private let lock = NSLock()
    private var fd: Int32
    let path: String

    private init(fd: Int32, path: String) {
        self.fd = fd
        self.path = path
    }

    deinit {
        if fd >= 0 { Darwin.close(fd) }
    }

    /// Connects to `path` after checking the directory and socket ownership, then verifies the peer uid.
    static func connect(path: String, verifyDirectory: Bool = true) throws(SocketConnectError) -> UnixSocket {
        let uid = getuid()
        let pathBytes = Array(path.utf8)
        guard pathBytes.count < DaemonSocket.sunPathCapacity else {
            throw .pathTooLong(path: path, bytes: pathBytes.count)
        }
        if verifyDirectory {
            try checkDirectory(of: path, uid: uid)
        }
        var info = stat()
        if lstat(path, &info) != 0 {
            let code = errno
            if code == ENOENT { throw .socketMissing(path: path) }
            throw .systemCall(name: "lstat", path: path, errno: code)
        }
        guard (info.st_mode & S_IFMT) == S_IFSOCK else { throw .notASocket(path: path) }
        guard info.st_uid == uid else { throw .socketOwnedByOtherUser(path: path, owner: info.st_uid) }

        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw .systemCall(name: "socket", path: path, errno: errno) }
        var ok = false
        defer { if !ok { Darwin.close(fd) } }

        _ = fcntl(fd, F_SETFD, FD_CLOEXEC)
        var one: Int32 = 1
        _ = setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &one, socklen_t(MemoryLayout<Int32>.size))
        // A stuck peer must not block a writer forever.
        var sendTimeout = timeval(tv_sec: 10, tv_usec: 0)
        _ = setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &sendTimeout, socklen_t(MemoryLayout<timeval>.size))

        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)
        withUnsafeMutableBytes(of: &address.sun_path) { raw in
            raw.copyBytes(from: pathBytes)
            raw[pathBytes.count] = 0
        }
        let result = withUnsafePointer(to: &address) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { sa in
                Darwin.connect(fd, sa, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        if result != 0 {
            let code = errno
            if code == ECONNREFUSED { throw .refused(path: path) }
            if code == ENOENT { throw .socketMissing(path: path) }
            throw .systemCall(name: "connect", path: path, errno: code)
        }

        var peerUID: uid_t = 0
        var peerGID: gid_t = 0
        guard getpeereid(fd, &peerUID, &peerGID) == 0 else {
            throw .systemCall(name: "getpeereid", path: path, errno: errno)
        }
        guard peerUID == uid else { throw .peerIdentityMismatch(path: path, expected: uid, actual: peerUID) }

        ok = true
        return UnixSocket(fd: fd, path: path)
    }

    /// The socket's directory must be a real directory (not a symlink), owned by us, with no group/other access.
    static func checkDirectory(of path: String, uid: uid_t) throws(SocketConnectError) {
        let directory = (path as NSString).deletingLastPathComponent
        var info = stat()
        if lstat(directory, &info) != 0 {
            let code = errno
            if code == ENOENT { throw .directoryMissing(path: directory) }
            throw .systemCall(name: "lstat", path: directory, errno: code)
        }
        let type = info.st_mode & S_IFMT
        if type == S_IFLNK { throw .directoryInsecure(path: directory, reason: "it is a symbolic link") }
        guard type == S_IFDIR else { throw .directoryInsecure(path: directory, reason: "it is not a directory") }
        guard info.st_uid == uid else {
            throw .directoryInsecure(path: directory, reason: "it is owned by uid \(info.st_uid), not uid \(uid)")
        }
        guard info.st_mode & 0o077 == 0 else {
            throw .directoryInsecure(
                path: directory, reason: "its mode is \(String(info.st_mode & 0o777, radix: 8)); group and others must have no access")
        }
    }

    /// Writes all of `data`. Serialized with other writers; fails once the socket was shut down.
    func send(_ data: Data) throws(SocketIOError) {
        lock.lock()
        defer { lock.unlock() }
        guard fd >= 0 else { throw .closed }
        let fd = self.fd
        let failure: Int32? = data.withUnsafeBytes { buffer -> Int32? in
            guard let base = buffer.baseAddress else { return nil }
            var offset = 0
            while offset < buffer.count {
                let n = Darwin.send(fd, base.advanced(by: offset), buffer.count - offset, 0)
                if n < 0 {
                    if errno == EINTR { continue }
                    return errno
                }
                offset += n
            }
            return nil
        }
        if let failure { throw .system(name: "send", errno: failure) }
    }

    /// Blocking read. Returns the byte count, 0 at end of stream, or -1 (with `errno`) on error.
    func receive(into buffer: UnsafeMutableRawBufferPointer) -> Int {
        let fd = lock.withLock { self.fd }
        guard fd >= 0, let base = buffer.baseAddress else { return 0 }
        while true {
            let n = Darwin.recv(fd, base, buffer.count, 0)
            if n < 0, errno == EINTR { continue }
            return n
        }
    }

    /// Wakes the reader and makes further writes fail. Safe from any thread, repeatable.
    func shutdown() {
        lock.withLock {
            if fd >= 0 { _ = Darwin.shutdown(fd, SHUT_RDWR) }
        }
    }

    /// Called by the reader thread when it is finished with the socket.
    func closeAfterReading() {
        lock.withLock {
            if fd >= 0 {
                Darwin.close(fd)
                fd = -1
            }
        }
    }
}

enum SocketIOError: Error, Sendable, Equatable, CustomStringConvertible {
    case closed
    case system(name: String, errno: Int32)

    var description: String {
        switch self {
        case .closed: "the socket to peekd is closed"
        case .system(let name, let code): "\(name)() failed: \(String(cString: strerror(code))) (errno \(code))"
        }
    }
}
