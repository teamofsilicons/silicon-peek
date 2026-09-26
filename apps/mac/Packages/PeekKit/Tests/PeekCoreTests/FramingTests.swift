import Foundation
import Testing

@testable import PeekCore

@Suite("Framing")
struct FramingTests {
    private func decodeAll(_ data: Data, chunk: Int = .max) throws -> [Frame] {
        var decoder = FrameDecoder()
        var frames: [Frame] = []
        var offset = 0
        while offset < data.count {
            let end = min(offset + chunk, data.count)
            decoder.append(data.subdata(in: offset..<end))
            offset = end
            while let frame = try decoder.next() { frames.append(frame) }
        }
        try decoder.finish()
        return frames
    }

    @Test("a frame with blobs round-trips, whole or fed one byte at a time")
    func roundTrip() throws {
        let blobs = [Data([1, 2, 3]), Data(), Data(repeating: 0xAB, count: 70_000)]
        let frame = Frame(fields: ["v": 1, "event": "tts.chunk", "send_id": "snd_1"], blobs: blobs)
        let encoded = try frame.encoded()
        let headerLine = String(decoding: encoded.prefix(while: { $0 != 0x0A }), as: UTF8.self)
        #expect(headerLine == #"{"bin":[3,0,70000],"event":"tts.chunk","send_id":"snd_1","v":1}"#)
        #expect(try decodeAll(encoded) == [frame])
        #expect(try decodeAll(encoded, chunk: 1) == [frame])
        #expect(try decodeAll(encoded, chunk: 4096) == [frame])
    }

    @Test("several frames in one read are all returned in order")
    func multipleFrames() throws {
        let a = Frame(fields: ["v": 1, "event": "a"])
        let b = Frame(fields: ["v": 1, "event": "b"], blobs: [Data("xyz".utf8)])
        let c = Frame(fields: ["v": 1, "event": "c"])
        var data = try a.encoded()
        data.append(try b.encoded())
        data.append(try c.encoded())
        #expect(try decodeAll(data) == [a, b, c])
    }

    @Test("the \"bin\" field never leaks into the header fields")
    func binStripped() throws {
        let frame = Frame(fields: ["v": 1, "bin": [5]])
        #expect(frame.fields["bin"] == nil)
        let decoded = try decodeAll(try Frame(fields: ["v": 1], blobs: [Data([9])]).encoded())
        #expect(decoded.first?.fields["bin"] == nil)
        #expect(decoded.first?.blobs == [Data([9])])
    }

    @Test("an over-long JSON line is a protocol error, even before its newline arrives")
    func lineTooLong() throws {
        var decoder = FrameDecoder()
        decoder.append(Data(repeating: 0x20, count: FrameLimits.maxLineBytes + 1))
        #expect(throws: FrameError.self) { try decoder.next() }
    }

    @Test(
        "invalid headers and bin declarations are protocol errors",
        arguments: [
            #"{"v":1,"a":1,"a":2}"#,
            #"[1,2]"#,
            #"{"v":1,"bin":"3"}"#,
            #"{"v":1,"bin":[-1]}"#,
            #"{"v":1,"bin":[1.5]}"#,
            #"{"v":1,"bin":[10485761]}"#,
            #"{"v":1,"bin":[10485760,10485760,10485760,10485760,1]}"#,
        ])
    func invalidHeaders(line: String) {
        var decoder = FrameDecoder()
        decoder.append(Data((line + "\n").utf8))
        #expect(throws: FrameError.self) { try decoder.next() }
    }

    @Test("a stream that ends mid-frame reports truncation")
    func truncated() throws {
        var decoder = FrameDecoder()
        decoder.append(Data(Array(#"{"v":1,"bin":[10]}"#.utf8) + [0x0A, 1, 2, 3]))
        #expect(try decoder.next() == nil)
        #expect(throws: FrameError.truncated(missingBytes: 7)) { try decoder.finish() }
    }

    @Test("encoding enforces the blob and binary limits")
    func encodeLimits() {
        let big = Frame(fields: ["v": 1], blobs: [Data(count: FrameLimits.maxBlobBytes + 1)])
        #expect(throws: FrameError.blobTooLarge(bytes: FrameLimits.maxBlobBytes + 1, limit: FrameLimits.maxBlobBytes)) {
            try big.encoded()
        }
        let many = Frame(fields: ["v": 1], blobs: Array(repeating: Data(), count: FrameLimits.maxBlobCount + 1))
        #expect(throws: FrameError.self) { try many.encoded() }
        let longText = Frame(fields: ["v": 1, "text": .string(String(repeating: "x", count: FrameLimits.maxLineBytes))])
        #expect(throws: FrameError.self) { try longText.encoded() }
    }
}
