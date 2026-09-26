import CoreGraphics
import PeekCore

/// What the menu bar lists: one row per registered slot, the free positions and the test contexts.
/// Pure data, derived from `slots.state` and settings, so it is unit-tested without any UI.
public struct MenuBarSummary: Equatable, Sendable {
    public struct Row: Identifiable, Equatable, Sendable {
        public var id: String
        public var index: SlotIndex
        public var name: String
        public var actorID: String
        /// `⌘3`, or nil when peekd asked the UI not to register this slot's hotkey.
        public var hotkey: String?
        /// `TEST · <environment>` for testing contexts.
        public var testPill: String?
        /// The environment UUID and generation, for the pill's tooltip.
        public var testTooltip: String?
        /// A testing bubble while "Show test peeks" is off: it waits in peekd.
        public var muted: Bool
        public var hasDrawing: Bool
    }

    public var rows: [Row]
    /// Positions no Silicon holds in any context.
    public var freePositions: [SlotIndex]
    /// Distinct testing environments with a registered slot.
    public var testEnvironmentCount: Int

    public init(slots: [SlotState], modifier: HotkeyModifier, showTestPeeks: Bool) {
        let sorted = slots.sorted { lhs, rhs in
            if lhs.index != rhs.index { return lhs.index < rhs.index }
            // Production first, then testing contexts by id.
            return (lhs.context == .production ? "" : lhs.context.rawValue) < (rhs.context == .production ? "" : rhs.context.rawValue)
        }
        rows = sorted.map { slot in
            var pill: String?
            var tooltip: String?
            var muted = false
            if case .testing(let environmentID) = slot.context {
                pill = "TEST · \(slot.environment?.name ?? "unnamed environment")"
                tooltip = "Testing environment \(environmentID)" + (slot.environment?.generation.map { ", generation \($0)" } ?? "")
                muted = !showTestPeeks
            } else if slot.context == .simulation {
                pill = "SIMULATION"
            }
            return Row(
                id: slot.id, index: slot.index, name: slot.displayName, actorID: slot.actorID,
                hotkey: slot.hotkey ? "\(modifier.symbols)\(slot.index.rawValue)" : nil, testPill: pill, testTooltip: tooltip,
                muted: muted, hasDrawing: slot.drawing != nil)
        }
        let taken = Set(slots.map(\.index))
        freePositions = SlotIndex.allCases.filter { !taken.contains($0) }
        testEnvironmentCount = Set(slots.compactMap { slot -> String? in
            if case .testing(let id) = slot.context { return id }
            return nil
        }).count
    }

    /// "Free positions: 2, 4, 6" / "All 8 positions are taken".
    public var freePositionsText: String {
        switch freePositions.count {
        case 0: "All 8 positions are taken."
        case 8: "All 8 positions are free."
        default: "Free positions: " + freePositions.map { String($0.rawValue) }.joined(separator: ", ")
        }
    }

    /// "1 test environment active" style line, or nil with none.
    public var testEnvironmentsText: String? {
        switch testEnvironmentCount {
        case 0: nil
        case 1: "1 testing environment active"
        default: "\(testEnvironmentCount) testing environments active"
        }
    }
}

/// Where each of the 8 positions sits in a small screen diagram (1 = top centre, clockwise).
public enum SlotDiagram {
    /// The centre of `index`'s marker inside `rect` (y-down, SwiftUI coordinates), inset by `inset`.
    public static func point(for index: SlotIndex, in rect: CGRect, inset: CGFloat) -> CGPoint {
        let minX = rect.minX + inset, maxX = rect.maxX - inset
        let minY = rect.minY + inset, maxY = rect.maxY - inset
        let midX = rect.midX, midY = rect.midY
        switch index.side {
        case .top: return CGPoint(x: midX, y: minY)
        case .topRight: return CGPoint(x: maxX, y: minY)
        case .right: return CGPoint(x: maxX, y: midY)
        case .bottomRight: return CGPoint(x: maxX, y: maxY)
        case .bottom: return CGPoint(x: midX, y: maxY)
        case .bottomLeft: return CGPoint(x: minX, y: maxY)
        case .left: return CGPoint(x: minX, y: midY)
        case .topLeft: return CGPoint(x: minX, y: minY)
        }
    }
}
