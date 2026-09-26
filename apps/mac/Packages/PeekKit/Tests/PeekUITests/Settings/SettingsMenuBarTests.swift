import CoreGraphics
import Foundation
import PeekCore
import Testing

@testable import PeekUI

@Suite("Menu bar summary")
struct SettingsMenuBarTests {
    private let environment = "0f8e2c4a-3333-4a5b-9c3d-000000000003"

    private var slots: [SlotState] {
        [
            SlotState(
                index: .bottom, context: .testing(environmentID: environment), actorID: "si:tester", orgID: "tos",
                displayName: "Tester", environment: TestEnvironmentInfo(id: environment, name: "peek testing", generation: 2)),
            SlotState(index: .bottom, actorID: "si:dj", orgID: "tos", displayName: "DJ",
                      drawing: DrawingRef(sha256: "abc", path: "/x.js")),
            SlotState(index: .top, actorID: "si:cleanup", orgID: "tos", hotkey: false),
        ]
    }

    @Test("rows are sorted by position, production first, with hotkeys in the chosen modifier")
    func rows() {
        let summary = MenuBarSummary(slots: slots, modifier: .ctrlCmd, showTestPeeks: true)
        #expect(summary.rows.map(\.index) == [.top, .bottom, .bottom])
        #expect(summary.rows.map(\.name) == ["cleanup", "DJ", "Tester"])
        #expect(summary.rows[0].hotkey == nil, "peekd asked for no hotkey")
        #expect(summary.rows[1].hotkey == "⌃⌘5")
        #expect(summary.rows[1].testPill == nil)
        #expect(summary.rows[1].hasDrawing)
        #expect(!summary.rows[0].hasDrawing)
    }

    @Test("testing rows carry the TEST pill with the environment in the tooltip")
    func testPill() throws {
        let visible = MenuBarSummary(slots: slots, modifier: .cmd, showTestPeeks: true)
        let row = try #require(visible.rows.last)
        #expect(row.testPill == "TEST · peek testing")
        #expect(row.testTooltip == "Testing environment \(environment), generation 2")
        #expect(!row.muted)
        #expect(visible.testEnvironmentCount == 1)
        #expect(visible.testEnvironmentsText == "1 testing environment active")

        let hidden = MenuBarSummary(slots: slots, modifier: .cmd, showTestPeeks: false)
        #expect(hidden.rows.last?.muted == true, "with Show test peeks off the row is dimmed")
        #expect(hidden.rows.first?.muted == false)
    }

    @Test("free positions ignore the context: a position is free only when nobody holds it")
    func freePositions() {
        let summary = MenuBarSummary(slots: slots, modifier: .cmd, showTestPeeks: true)
        #expect(summary.freePositions.map(\.rawValue) == [2, 3, 4, 6, 7, 8])
        #expect(summary.freePositionsText == "Free positions: 2, 3, 4, 6, 7, 8")
        #expect(MenuBarSummary(slots: [], modifier: .cmd, showTestPeeks: true).freePositionsText == "All 8 positions are free.")
        let full = SlotIndex.allCases.map { SlotState(index: $0, actorID: "si:s\($0.rawValue)", orgID: "tos") }
        let summaryFull = MenuBarSummary(slots: full, modifier: .cmd, showTestPeeks: true)
        #expect(summaryFull.freePositionsText == "All 8 positions are taken.")
        #expect(summaryFull.testEnvironmentsText == nil)
    }

    @Test("the position diagram puts 1 at top centre and goes clockwise")
    func diagram() {
        let rect = CGRect(x: 0, y: 0, width: 100, height: 60)
        let points = SlotIndex.allCases.map { SlotDiagram.point(for: $0, in: rect, inset: 5) }
        #expect(points[0] == CGPoint(x: 50, y: 5))
        #expect(points[1] == CGPoint(x: 95, y: 5))
        #expect(points[2] == CGPoint(x: 95, y: 30))
        #expect(points[3] == CGPoint(x: 95, y: 55))
        #expect(points[4] == CGPoint(x: 50, y: 55))
        #expect(points[5] == CGPoint(x: 5, y: 55))
        #expect(points[6] == CGPoint(x: 5, y: 30))
        #expect(points[7] == CGPoint(x: 5, y: 5))
        #expect(points.allSatisfy { rect.contains($0) })
    }
}
