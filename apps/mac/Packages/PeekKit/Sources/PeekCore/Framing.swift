import Foundation

/// Size limits of IPC protocol v1 (BLUEPRINT §1.6 "Framing").
public enum FrameLimits {
    /// A JSON line, excluding its `\n`.
    public static let maxLineBytes = 1 << 20
    /// Any single blob. Images are the largest kind (10 MiB).
    public static let maxBlobBytes = 10 << 20
    /// All blobs of one frame together.
    public static let maxBinaryBytesPerFrame = 40 << 20
    /// Blobs in one frame. Not in the blueprint; 64 is far above any v1 message (a `send` has ≤ 3 + 6 images).
    public static let maxBlobCount = 64

    public static let maxImageBytes = 10 << 20
    public static let maxDrawingBytes = 256 << 10
    /// 120 s of 16 kHz mono Int16 plus the header.
    public static let maxWAVBytes = 4 << 20
    public static let maxTTSChunkBytes = 64 << 10
}

/// One protocol frame: a JSON object header plus the binary blobs it declares in `"bin"`.
///
/// On the wire the header is one UTF-8 line; when `blobs` is non-empty it carries
/// `"bin":[n1,n2,…]` and exactly `n1+n2+…` raw bytes follow the `\n`.
public struct Frame: Sendable, Equatable {
    /// Top-level header fields. `"bin"` is managed by the framing layer and is never stored here.
    public var fields: [String: JSONValue]
    public var blobs: [Data]

    public init(fields: [String: JSONValue], blobs: [Data] = []) {
        var fields = fields
        fields.removeValue(forKey: "bin")
        self.fields = fields
        self.blobs = blobs
    }

    public subscript(key: String) -> JSONValue? { fields[key] }

    /// The header as a JSON object (without `"bin"`).
    public var header: JSONValue { .object(fields) }

    /// Serializes the frame for the wire, enforcing ``FrameLimits``.
    public func encoded() throws(FrameError) -> Data {
        var header = fields
        if !blobs.isEmpty {
            guard blobs.count <= FrameLimits.maxBlobCount else {
                throw .tooManyBlobs(count: blobs.count, limit: FrameLimits.maxBlobCount)
            }
            var total = 0
            for blob in blobs {
                guard blob.count <= FrameLimits.maxBlobBytes else {
                    throw .blobTooLarge(bytes: blob.count, limit: FrameLimits.maxBlobBytes)
                }
                total += blob.count
            }
            guard total <= FrameLimits.maxBinaryBytesPerFrame else {
                throw .binaryTooLarge(bytes: total, limit: FrameLimits.maxBinaryBytesPerFrame)
            }
            header["bin"] = .array(blobs.map { .int(Int64($0.count)) })
        }
        var line: [UInt8] = []
        line.reserveCapacity(256)
        JSONValue.object(header).write(into: &line)
        guard line.count <= FrameLimits.maxLineBytes else {
            throw .lineTooLong(bytes: line.count, limit: FrameLimits.maxLineBytes)
        }
        var data = Data(line)
        data.append(0x0A)
        for blob in blobs { data.append(blob) }
        return data
    }
}

/// Why a frame could not be encoded or decoded. Decoding errors are protocol
/// violations: the connection that produced them must be closed.
public enum FrameError: Error, Equatable, Sendable, CustomStringConvertible {
    case lineTooLong(bytes: Int, limit: Int)
    case invalidJSON(StrictJSON.ParseError)
    case headerNotObject
    case invalidBinDeclaration(String)
    case tooManyBlobs(count: Int, limit: Int)
    case blobTooLarge(bytes: Int, limit: Int)
    case binaryTooLarge(bytes: Int, limit: Int)
    case truncated(missingBytes: Int)

    public var description: String {
        switch self {
        case .lineTooLong(let bytes, let limit):
            return "frame header is \(bytes) bytes; the protocol limit is \(limit) bytes per JSON line"
        case .invalidJSON(let error):
            return "frame header is not strict JSON: \(error)"
        case .headerNotObject:
            return "frame header must be a JSON object"
        case .invalidBinDeclaration(let why):
            return "frame \"bin\" declaration is invalid: \(why)"
        case .tooManyBlobs(let count, let limit):
            return "frame declares \(count) blobs; at most \(limit) are allowed"
        case .blobTooLarge(let bytes, let limit):
            return "frame blob is \(bytes) bytes; the limit is \(limit) bytes per blob"
        case .binaryTooLarge(let bytes, let limit):
            return "frame carries \(bytes) binary bytes; the limit is \(limit) bytes per frame"
        case .truncated(let missing):
            return "connection closed \(missing) bytes before the end of a frame"
        }
    }
}

/// Incremental decoder for the NDJSON + binary framing. Feed it bytes as they
/// arrive with ``append(_:)`` and drain complete frames with ``next()``.
public struct FrameDecoder: Sendable {
    private var buffer = Data()
    private var pending: (fields: [String: JSONValue], sizes: [Int], total: Int)?
    /// Bytes of `buffer` already scanned for a newline without finding one.
    private var scanned = 0

    public init() {}

    /// Bytes received but not yet returned as part of a frame.
    public var bufferedByteCount: Int { buffer.count }

    public mutating func append(_ bytes: Data) { buffer.append(bytes) }

    public mutating func append(_ bytes: UnsafeRawBufferPointer) { buffer.append(contentsOf: bytes) }

    /// Returns the next complete frame, `nil` when more bytes are needed, or
    /// throws on a protocol violation (after which the decoder must be discarded).
    public mutating func next() throws(FrameError) -> Frame? {
        if pending == nil {
            guard let newline = findNewline() else {
                if buffer.count > FrameLimits.maxLineBytes {
                    throw .lineTooLong(bytes: buffer.count, limit: FrameLimits.maxLineBytes)
                }
                return nil
            }
            let lineLength = newline - buffer.startIndex
            guard lineLength <= FrameLimits.maxLineBytes else {
                throw .lineTooLong(bytes: lineLength, limit: FrameLimits.maxLineBytes)
            }
            let line = [UInt8](buffer[buffer.startIndex..<newline])
            buffer.removeSubrange(buffer.startIndex...newline)
            scanned = 0

            let value: JSONValue
            do {
                value = try StrictJSON.parse(bytes: line)
            } catch {
                throw .invalidJSON(error)
            }
            guard case .object(var fields) = value else { throw .headerNotObject }
            let bin = fields.removeValue(forKey: "bin")
            let sizes = try Self.blobSizes(bin)
            let total = sizes.reduce(0, +)
            pending = (fields, sizes, total)
        }

        guard let (fields, sizes, total) = pending else { return nil }
        guard buffer.count >= total else { return nil }
        var blobs: [Data] = []
        blobs.reserveCapacity(sizes.count)
        var cursor = buffer.startIndex
        for size in sizes {
            blobs.append(Data(buffer[cursor..<cursor + size]))
            cursor += size
        }
        buffer.removeSubrange(buffer.startIndex..<cursor)
        pending = nil
        return Frame(fields: fields, blobs: blobs)
    }

    /// Call when the stream ended: throws if a frame was cut off mid-way.
    public func finish() throws(FrameError) {
        if let pending {
            throw .truncated(missingBytes: pending.total - buffer.count)
        }
        if !buffer.isEmpty {
            throw .truncated(missingBytes: 1)
        }
    }

    private mutating func findNewline() -> Data.Index? {
        let start = buffer.startIndex + scanned
        if let index = buffer[start...].firstIndex(of: 0x0A) {
            return index
        }
        scanned = buffer.count
        return nil
    }

    private static func blobSizes(_ bin: JSONValue?) throws(FrameError) -> [Int] {
        guard let bin else { return [] }
        guard case .array(let entries) = bin else {
            throw .invalidBinDeclaration("expected an array of byte counts")
        }
        guard entries.count <= FrameLimits.maxBlobCount else {
            throw .tooManyBlobs(count: entries.count, limit: FrameLimits.maxBlobCount)
        }
        var sizes: [Int] = []
        var total = 0
        for entry in entries {
            guard case .int(let raw) = entry, raw >= 0, let size = Int(exactly: raw) else {
                throw .invalidBinDeclaration("each entry must be a non-negative integer")
            }
            guard size <= FrameLimits.maxBlobBytes else {
                throw .blobTooLarge(bytes: size, limit: FrameLimits.maxBlobBytes)
            }
            total += size
            guard total <= FrameLimits.maxBinaryBytesPerFrame else {
                throw .binaryTooLarge(bytes: total, limit: FrameLimits.maxBinaryBytesPerFrame)
            }
            sizes.append(size)
        }
        return sizes
    }
}
