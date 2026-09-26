import Foundation
import PeekCore
import Testing

@testable import PeekDrawing

@Suite("engine smoke")
struct EngineSmokeTests {
    @Test("the prelude loads and records a simple frame")
    func smoke() async throws {
        let harness = try await EngineHarness()
        let status = await harness.load("""
            peek.frame((ctx, input) => {
              ctx.fillStyle = 'red'
              ctx.beginPath(); ctx.arc(50, 50, 20, 0, Math.PI * 2); ctx.fill()
              return true
            })
            """)
        #expect(status == .ok, "\(status)")
        let (frame, list) = await harness.decode(dump: true)
        #expect(frame.status == .ok, "\(frame.status)")
        #expect(frame.again)
        let names = opNames(try #require(list))
        #expect(names == ["fillStyle", "beginPath", "arc", "fill"], "\(names)")
        print(await harness.logs)
    }
}
