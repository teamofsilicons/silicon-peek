import Foundation
import Testing

@testable import PeekCore

@Suite("ui.log")
struct LoggingTests {
    @Test("timestamps are UTC with a Z suffix and milliseconds, like peekd.log")
    func utcTimestamps() {
        let date = Date(timeIntervalSince1970: 1_790_419_429.25)
        #expect(RotatingLogFile.timestamp(date) == "2026-09-26T10:43:49.250Z")
    }

    @Test("a line carries the UTC timestamp, the level, the category and one-line text")
    func lineFormat() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("peek-log-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: dir) }
        let file = RotatingLogFile(url: dir.appendingPathComponent("ui.log"))
        file.append(level: .info, category: "slots", message: "show snd_1 at slot 5: speak\nsecond line",
                    date: Date(timeIntervalSince1970: 1_790_419_429.5))
        let text = try String(contentsOf: file.url, encoding: .utf8)
        #expect(text == "2026-09-26T10:43:49.500Z INFO [slots] show snd_1 at slot 5: speak ⏎ second line\n")
    }
}
