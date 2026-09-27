import Foundation
import PeekCore

/// Who an Esc belongs to, and whether Peek needs Esc at all right now (peek 0.1.2, agreed #12–#15; pure, so the rules
/// are testable without Carbon). ``SlotManager`` holds bare Esc (``EscapeKeyProviding``) only while ``needsEsc`` is
/// true and no Peek window is key (a key slot panel, Settings or Simulation gets Esc through AppKit instead):
///
///   1. a slot's tap-to-expand popup is open;
///   2. a bubble slid in less than 3 s ago (its grace window; every appearance, the next queued one included, opens
///      its own, never extended);
///   3. the pointer is over a bubble's content (hover-Esc);
///   4. an Esc window the Carbon opened with an Esc to a bubble is still open (the 0.4 s double-Esc window, the 2 s
///      "Esc again to dismiss" hint). Peek never opens one by itself.
///
/// A press goes to (1) the slot with an open popup (it only closes the popup), else (2) the bubble with the latest open
/// Esc window, else (3) the hovered bubble, else (4) the bubble that slid in most recently.
public enum EscapeRouting {
    public struct Candidate: Sendable, Equatable {
        public var slot: SlotIndex
        /// The slot's tap-to-expand popup is open.
        public var popupOpen: Bool
        /// The bubble is sliding in or visible (Esc applies to it).
        public var onScreen: Bool
        public var graceUntil: Double?
        public var armedUntil: Double?
        /// The pointer is over the bubble's content.
        public var hovered: Bool
        /// When it began to slide in.
        public var slidInAt: Double?

        public init(slot: SlotIndex, popupOpen: Bool = false, onScreen: Bool = false, graceUntil: Double? = nil,
                    armedUntil: Double? = nil, hovered: Bool = false, slidInAt: Double? = nil) {
            self.slot = slot
            self.popupOpen = popupOpen
            self.onScreen = onScreen
            self.graceUntil = graceUntil
            self.armedUntil = armedUntil
            self.hovered = hovered
            self.slidInAt = slidInAt
        }

        func inGrace(_ now: Double) -> Bool { onScreen && (graceUntil.map { now < $0 } ?? false) }
        func armed(_ now: Double) -> Bool { armedUntil.map { now < $0 } ?? false }
        var hoveredOnScreen: Bool { hovered && onScreen }
    }

    public enum Target: Sendable, Equatable {
        /// Close this slot's popup (and nothing else).
        case popup(SlotIndex)
        /// Deliver `.escape` to this slot's bubble.
        case bubble(SlotIndex)
    }

    public static func needsEsc(_ candidates: [Candidate], now: Double) -> Bool {
        candidates.contains { $0.popupOpen || $0.inGrace(now) || $0.hoveredOnScreen || $0.armed(now) }
    }

    /// The target of a press that came through the global hot key.
    public static func target(_ candidates: [Candidate], now: Double) -> Target? {
        if let popup = candidates.first(where: \.popupOpen) { return .popup(popup.slot) }
        if let armed = latestArmed(candidates, now: now) { return .bubble(armed.slot) }
        if let hovered = candidates.first(where: \.hoveredOnScreen) { return .bubble(hovered.slot) }
        let recent = candidates.filter(\.onScreen).max { ($0.slidInAt ?? -.infinity) < ($1.slidInAt ?? -.infinity) }
        return recent.map { .bubble($0.slot) }
    }

    /// The target of an Esc typed into a key slot panel (typing or recording there): a popup first, then a bubble whose
    /// Esc window is open, else that panel's own bubble.
    public static func keyPanelTarget(_ candidates: [Candidate], keySlot: SlotIndex, now: Double) -> Target? {
        if let popup = candidates.first(where: \.popupOpen) { return .popup(popup.slot) }
        if let armed = latestArmed(candidates, now: now) { return .bubble(armed.slot) }
        return .bubble(keySlot)
    }

    /// The earliest grace or Esc-window end after `now` (when ``needsEsc`` may change by itself).
    public static func nextDeadline(_ candidates: [Candidate], now: Double) -> Double? {
        candidates.flatMap { candidate -> [Double] in
            [candidate.onScreen ? candidate.graceUntil : nil, candidate.armedUntil].compactMap { $0 }.filter { $0 > now }
        }.min()
    }

    /// How recent the newest target is (orders several Esc clients in one app).
    public static func priority(_ candidates: [Candidate], now: Double) -> Double {
        if candidates.contains(where: \.popupOpen) { return .infinity }
        if let armed = latestArmed(candidates, now: now)?.armedUntil { return 1e12 + armed }
        if candidates.contains(where: \.hoveredOnScreen) { return 1e11 }
        return candidates.filter(\.onScreen).compactMap(\.slidInAt).max() ?? -.infinity
    }

    private static func latestArmed(_ candidates: [Candidate], now: Double) -> Candidate? {
        candidates.filter { $0.armed(now) }.max { ($0.armedUntil ?? 0) < ($1.armedUntil ?? 0) }
    }
}
