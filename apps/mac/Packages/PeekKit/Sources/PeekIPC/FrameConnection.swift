import Dispatch
import Foundation
import PeekCore

/// One socket connection that speaks frames: a dedicated reader thread decodes
/// incoming bytes (strict JSON, §1.6 limits) into an ordered stream, and writes
/// are serialized on a private queue so a large blob never blocks the caller.
final class FrameConnection: @unchecked Sendable {
    enum Input: Sendable {
        case frame(Frame)
        /// The connection ended; the string says why (EOF, read error or protocol violation).
        case closed(String)
    }

    let socket: UnixSocket
    let inputs: AsyncStream<Input>
    private let writeQueue: DispatchQueue

    static let readChunkBytes = 64 << 10

    init(socket: UnixSocket) {
        self.socket = socket
        self.writeQueue = DispatchQueue(label: "ai.tos.peek.ipc.write", qos: .userInitiated)
        let (stream, continuation) = AsyncStream.makeStream(of: Input.self, bufferingPolicy: .unbounded)
        self.inputs = stream
        let thread = Thread { [socket] in
            Self.readLoop(socket: socket, continuation: continuation)
        }
        thread.name = "ai.tos.peek.ipc.reader"
        thread.qualityOfService = .userInitiated
        thread.start()
    }

    static func open(path: String, verifyDirectory: Bool) throws(SocketConnectError) -> FrameConnection {
        FrameConnection(socket: try UnixSocket.connect(path: path, verifyDirectory: verifyDirectory))
    }

    private static func readLoop(socket: UnixSocket, continuation: AsyncStream<Input>.Continuation) {
        var decoder = FrameDecoder()
        let buffer = UnsafeMutableRawBufferPointer.allocate(byteCount: readChunkBytes, alignment: 16)
        defer {
            buffer.deallocate()
            socket.closeAfterReading()
            continuation.finish()
        }
        while true {
            let count = socket.receive(into: buffer)
            if count > 0 {
                decoder.append(UnsafeRawBufferPointer(rebasing: buffer[0..<count]))
                do {
                    while let frame = try decoder.next() {
                        continuation.yield(.frame(frame))
                    }
                } catch {
                    continuation.yield(.closed("peekd sent an invalid frame (\(error)); reconnecting"))
                    return
                }
            } else if count == 0 {
                do {
                    try decoder.finish()
                    continuation.yield(.closed("peekd closed the connection"))
                } catch {
                    continuation.yield(.closed("peekd closed the connection mid-frame (\(error))"))
                }
                return
            } else {
                let code = errno
                continuation.yield(.closed("reading from peekd failed: \(String(cString: strerror(code))) (errno \(code))"))
                return
            }
        }
    }

    /// Writes an encoded frame. Returns when the bytes are handed to the kernel.
    func write(_ data: Data) async -> Result<Void, SocketIOError> {
        await withCheckedContinuation { continuation in
            writeQueue.async { [socket] in
                do throws(SocketIOError) {
                    try socket.send(data)
                    continuation.resume(returning: .success(()))
                } catch {
                    continuation.resume(returning: .failure(error))
                }
            }
        }
    }

    /// Ends the connection; the reader emits `.closed` and finishes the stream.
    func close() { socket.shutdown() }
}
