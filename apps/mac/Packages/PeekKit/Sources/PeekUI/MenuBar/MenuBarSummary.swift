import CoreGraphics
import PeekCore

/// The registered accounts and available positions shown in the menu bar.
public struct MenuBarSummary: Equatable, Sendable {
    public struct Row: Identifiable, Equatable, Sendable {
        public var id: String
        public var index: SlotIndex
        public var name: String
        public var actorID: String
        public var hotkey: String?
        public var simulation: Bool
        public var hasDrawing: Bool
    }

    public var rows: [Row]
    public var freePositions: [SlotIndex]

    public init(slots: [SlotState], modifier: HotkeyModifier) {
        rows = slots.sorted { lhs, rhs in
            if lhs.index != rhs.index { return lhs.index < rhs.index }
            return SlotPriority.of(lhs.context) > SlotPriority.of(rhs.context)
        }.map { slot in
            Row(id: slot.id, index: slot.index, name: slot.displayName, actorID: slot.actorID,
                hotkey: slot.hotkey ? "\(modifier.symbols)\(slot.index.rawValue)" : nil,
                simulation: slot.context == .simulation, hasDrawing: slot.drawing != nil)
        }
        let taken = Set(slots.map(\.index))
        freePositions = SlotIndex.allCases.filter { !taken.contains($0) }
    }

    public var freePositionsText: String {
        switch freePositions.count {
        case 0: "All 8 positions are taken."
        case 8: "All 8 positions are free."
        default: "Free positions: " + freePositions.map { String($0.rawValue) }.joined(separator: ", ")
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
