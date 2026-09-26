import Foundation
import PeekCore
import Testing

@testable import PeekUI

@Suite("peekd log tail")
struct SettingsLogTailTests {
    private func temporaryFile(_ contents: Data) throws -> URL {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("peek-log-\(UUID().uuidString).log")
        try contents.write(to: url)
        return url
    }

    @Test("a missing log says where peekd writes it and what that means")
    func missing() {
        let url = URL(fileURLWithPath: "/nonexistent/peek-\(UUID().uuidString)/peekd.log")
        #expect(throws: LogTailReader.Failure.missing(path: url.path)) { try LogTailReader.read(url) }
        #expect(LogTailReader.Failure.missing(path: url.path).description.contains("peekd creates it when it starts"))
    }

    @Test("a small log is returned whole")
    func small() throws {
        let url = try temporaryFile(Data("one\ntwo\r\nthree\n".utf8))
        defer { try? FileManager.default.removeItem(at: url) }
        let tail = try LogTailReader.read(url)
        #expect(tail.lines == ["one", "two", "three"])
        #expect(!tail.truncated)
        #expect(tail.fileSize == 15)
        #expect(tail.text == "one\ntwo\nthree")
    }

    @Test("a large log keeps only whole lines from its end")
    func largeByBytes() throws {
        let lines = (1...2_000).map { String(format: "line %04d", $0) }
        let url = try temporaryFile(Data((lines.joined(separator: "\n") + "\n").utf8))
        defer { try? FileManager.default.removeItem(at: url) }
        let tail = try LogTailReader.read(url, maxBytes: 105, maxLines: 1_000)
        #expect(tail.truncated)
        #expect(tail.lines.last == "line 2000")
        #expect(tail.lines.allSatisfy { $0.hasPrefix("line ") && $0.count == 9 }, "the cut first line is dropped")
        #expect(tail.lines.count == 10)
    }

    @Test("maxLines caps the number of lines")
    func largeByLines() throws {
        let url = try temporaryFile(Data((1...50).map { "entry \($0)" }.joined(separator: "\n").utf8))
        defer { try? FileManager.default.removeItem(at: url) }
        let tail = try LogTailReader.read(url, maxLines: 5)
        #expect(tail.lines == ["entry 46", "entry 47", "entry 48", "entry 49", "entry 50"])
        #expect(tail.truncated)
    }

    @Test("invalid UTF-8 is replaced, not fatal")
    func invalidUTF8() throws {
        var data = Data("ok\n".utf8)
        data.append(contentsOf: [0xFF, 0xFE, 0x0A])
        data.append(Data("after\n".utf8))
        let url = try temporaryFile(data)
        defer { try? FileManager.default.removeItem(at: url) }
        let tail = try LogTailReader.read(url)
        #expect(tail.lines.count == 3)
        #expect(tail.lines.first == "ok")
        #expect(tail.lines.last == "after")
    }
}
