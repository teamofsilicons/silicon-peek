import Foundation
import PeekCore

/// Live account activity takes precedence over the local simulation.
public enum SlotPriority {
    public static func of(_ context: PeekContext) -> Int {
        switch context {
        case .production: 2
        case .simulation: 1
        }
    }
}

public struct SlotGate: Sendable, Equatable {
    public var paused: Bool

    public init(paused: Bool = false) {
        self.paused = paused
    }
}

public struct SlotScheduler: Sendable, Equatable {
    public struct Entry: Sendable, Equatable {
        public var id: BubbleID
        public var key: SiliconKey
        public var context: PeekContext
        public var expectsAudio: Bool
        /// The Carbon asked for it (hotkey): it ignores pause.
        public var summoned: Bool
        /// Arrival order; re-queued (pre-empted) bubbles get a negative one so they go first.
        public var sequence: Int

        public init(id: BubbleID, key: SiliconKey, context: PeekContext, expectsAudio: Bool, summoned: Bool = false,
                    sequence: Int) {
            self.id = id
            self.key = key
            self.context = context
            self.expectsAudio = expectsAudio
            self.summoned = summoned
            self.sequence = sequence
        }

        public var priority: Int { SlotPriority.of(context) }
    }

    /// What the slot shows right now.
    public struct Active: Sendable, Equatable {
        public var id: BubbleID
        public var key: SiliconKey
        public var context: PeekContext
        public var isAskOpen: Bool
        public var hasUserInput: Bool
        public var isLeaving: Bool
        public var isSummon: Bool

        public init(id: BubbleID, key: SiliconKey, context: PeekContext, isAskOpen: Bool, hasUserInput: Bool,
                    isLeaving: Bool, isSummon: Bool) {
            self.id = id
            self.key = key
            self.context = context
            self.isAskOpen = isAskOpen
            self.hasUserInput = hasUserInput
            self.isLeaving = isLeaving
            self.isSummon = isSummon
        }

        public var priority: Int { SlotPriority.of(context) }
    }

    public enum Decision: Sendable, Equatable {
        /// Nothing is in the way: slide it in now.
        case present
        /// Wait in the queue.
        case enqueue
        /// The active bubble (an idle summon of the same Silicon) slides out; this one follows.
        case replaceActive
        /// The active bubble (lower priority) slides out and is queued again; this one follows.
        case preemptActive
    }

    public private(set) var queue: [Entry] = []

    public init() {}

    /// Decides what to do with a bubble that just arrived. `audioPlaying` is true while a dismissed
    /// bubble's speech still plays in this slot.
    public func decide(_ entry: Entry, active: Active?, gate: SlotGate, audioPlaying: Bool) -> Decision {
        // A queued send from the same Silicon goes first (sends keep their order).
        let blocked: Set<SiliconKey> = queue.contains { $0.key == entry.key && $0.id != entry.id } ? [entry.key] : []
        guard isEligible(entry, gate: gate, audioPlaying: audioPlaying, blockedKeys: blocked) else { return .enqueue }
        guard let active else { return .present }
        if active.isLeaving { return .enqueue }
        if entry.priority > active.priority {
            return active.hasUserInput ? .enqueue : .preemptActive
        }
        if entry.priority < active.priority { return .enqueue }
        guard entry.key == active.key else { return .enqueue }
        // Same Silicon: its sends queue in order. An idle summon (the Carbon opened an empty bubble) makes way.
        if active.isSummon, !active.hasUserInput { return .replaceActive }
        return .enqueue
    }

    /// Adds `entry` in priority order (FIFO within a priority; negative sequences first).
    public mutating func enqueue(_ entry: Entry) {
        queue.removeAll { $0.id == entry.id }
        let index = queue.firstIndex { other in
            other.priority < entry.priority || (other.priority == entry.priority && other.sequence > entry.sequence)
        } ?? queue.endIndex
        queue.insert(entry, at: index)
    }

    @discardableResult
    public mutating func remove(_ id: BubbleID) -> Entry? {
        guard let index = queue.firstIndex(where: { $0.id == id }) else { return nil }
        return queue.remove(at: index)
    }

    public func contains(_ id: BubbleID) -> Bool { queue.contains { $0.id == id } }

    /// Removes and returns the next bubble that may be shown, if any.
    public mutating func popNext(gate: SlotGate, audioPlaying: Bool) -> Entry? {
        var blocked = Set<SiliconKey>()
        for (index, entry) in queue.enumerated() {
            if isEligible(entry, gate: gate, audioPlaying: audioPlaying, blockedKeys: blocked) {
                return queue.remove(at: index)
            }
            blocked.insert(entry.key)
        }
        return nil
    }

    /// Marks an entry as summoned (the Carbon pressed the hotkey) and moves it to the front of its priority.
    public mutating func summon(_ id: BubbleID) {
        guard var entry = remove(id) else { return }
        entry.summoned = true
        entry.sequence = Int.min / 2
        enqueue(entry)
    }

    /// The first queued bubble for `key`, if any.
    public func firstQueued(for key: SiliconKey) -> Entry? { queue.first { $0.key == key } }

    // MARK: Rules

    func isEligible(_ entry: Entry, gate: SlotGate, audioPlaying: Bool, blockedKeys: Set<SiliconKey>) -> Bool {
        if blockedKeys.contains(entry.key) { return false }
        if entry.expectsAudio && audioPlaying { return false }
        if entry.summoned { return true }
        if gate.paused && entry.context != .simulation { return false }
        return true
    }
}
