import Foundation
import PeekCore
import Testing

@testable import PeekUI

@Suite("Slot scheduler: queue and priority rules")
struct SlotSchedulerTests {
    let prod = SiliconKey(context: .production, orgID: "tos", actorID: "si:dj")
    let other = SiliconKey(context: .production, orgID: "tos", actorID: "si:other")
    let test = SiliconKey(context: .testing(environmentID: "env"), orgID: "tos", actorID: "si:dj")
    let sim = SiliconKey(context: .simulation, orgID: "simulation", actorID: "si:simulation")

    func entry(_ id: String, _ key: SiliconKey, audio: Bool = false, summoned: Bool = false, seq: Int = 1)
        -> SlotScheduler.Entry
    {
        SlotScheduler.Entry(id: BubbleID(id), key: key, context: key.context, expectsAudio: audio, summoned: summoned,
                            sequence: seq)
    }

    func active(_ id: String, _ key: SiliconKey, ask: Bool = false, input: Bool = false, leaving: Bool = false,
                summon: Bool = false) -> SlotScheduler.Active {
        SlotScheduler.Active(id: BubbleID(id), key: key, context: key.context, isAskOpen: ask, hasUserInput: input,
                             isLeaving: leaving, isSummon: summon)
    }

    @Test("an empty slot presents at once")
    func presentsWhenFree() {
        let scheduler = SlotScheduler()
        #expect(scheduler.decide(entry("a", prod), active: nil, gate: SlotGate(), audioPlaying: false) == .present)
    }

    @Test("peek 0.1.2: a new send from the same Silicon queues behind its visible show; only an idle summon makes way")
    func sameSiliconQueues() {
        let scheduler = SlotScheduler()
        #expect(scheduler.decide(entry("b", prod), active: active("a", prod), gate: SlotGate(), audioPlaying: false)
            == .enqueue)
        #expect(scheduler.decide(entry("b", prod), active: active("s", prod, summon: true), gate: SlotGate(),
                                 audioPlaying: false) == .replaceActive)
        #expect(scheduler.decide(entry("b", prod), active: active("s", prod, input: true, summon: true), gate: SlotGate(),
                                 audioPlaying: false) == .enqueue, "a summon the Carbon is typing into keeps its place")
    }

    @Test("sends queue behind a pending ask, behind typing or recording, and behind a leaving bubble")
    func queuesBehindAskAndInput() {
        let scheduler = SlotScheduler()
        let gate = SlotGate()
        #expect(scheduler.decide(entry("b", prod), active: active("a", prod, ask: true), gate: gate, audioPlaying: false)
            == .enqueue)
        #expect(scheduler.decide(entry("b", prod), active: active("a", prod, input: true), gate: gate, audioPlaying: false)
            == .enqueue)
        #expect(scheduler.decide(entry("b", prod), active: active("a", prod, leaving: true), gate: gate, audioPlaying: false)
            == .enqueue)
    }

    @Test("production pre-empts a visible test bubble; a test peek waits while production shows")
    func productionPriority() {
        let scheduler = SlotScheduler()
        let gate = SlotGate()
        #expect(scheduler.decide(entry("p", prod), active: active("t", test, ask: true), gate: gate, audioPlaying: false)
            == .preemptActive)
        #expect(scheduler.decide(entry("t", test), active: active("p", prod), gate: gate, audioPlaying: false) == .enqueue)
        // Not while the Carbon is typing an answer into the test bubble.
        #expect(scheduler.decide(entry("p", prod), active: active("t", test, input: true), gate: gate, audioPlaying: false)
            == .enqueue)
        // Simulation sits between the two.
        #expect(scheduler.decide(entry("s", sim), active: active("t", test), gate: gate, audioPlaying: false) == .preemptActive)
        #expect(scheduler.decide(entry("s", sim), active: active("p", prod), gate: gate, audioPlaying: false) == .enqueue)
    }

    @Test("pause queues Silicon peeks (not Simulation); 'Show test peeks' off queues test peeks silently")
    func gates() {
        let scheduler = SlotScheduler()
        #expect(scheduler.decide(entry("a", prod), active: nil, gate: SlotGate(paused: true), audioPlaying: false) == .enqueue)
        #expect(scheduler.decide(entry("s", sim), active: nil, gate: SlotGate(paused: true), audioPlaying: false) == .present)
        #expect(scheduler.decide(entry("t", test), active: nil, gate: SlotGate(showTestPeeks: false), audioPlaying: false)
            == .enqueue)
        #expect(scheduler.decide(entry("a", prod), active: nil, gate: SlotGate(showTestPeeks: false), audioPlaying: false)
            == .present)
        // A hotkey summon shows it anyway.
        #expect(scheduler.decide(entry("t", test, summoned: true), active: nil, gate: SlotGate(paused: true, showTestPeeks: false),
                                 audioPlaying: false) == .present)
    }

    @Test("a send with speech waits while a dismissed bubble's audio still plays; a silent one does not")
    func waitsForAudio() {
        let scheduler = SlotScheduler()
        #expect(scheduler.decide(entry("a", prod, audio: true), active: nil, gate: SlotGate(), audioPlaying: true) == .enqueue)
        #expect(scheduler.decide(entry("a", prod), active: nil, gate: SlotGate(), audioPlaying: true) == .present)
    }

    @Test("the queue orders by priority, then arrival; re-queued bubbles go first within their priority")
    func ordering() {
        var scheduler = SlotScheduler()
        scheduler.enqueue(entry("t1", test, seq: 1))
        scheduler.enqueue(entry("p1", prod, seq: 2))
        scheduler.enqueue(entry("t2", test, seq: 3))
        scheduler.enqueue(entry("p2", prod, seq: 4))
        scheduler.enqueue(entry("t0", test, seq: -5))
        #expect(scheduler.queue.map(\.id.raw) == ["p1", "p2", "t0", "t1", "t2"])
        #expect(scheduler.popNext(gate: SlotGate(), audioPlaying: false)?.id.raw == "p1")
        #expect(scheduler.remove(BubbleID("t1"))?.id.raw == "t1")
        #expect(scheduler.queue.map(\.id.raw) == ["p2", "t0", "t2"])
    }

    @Test("sends from one Silicon keep their order; blocked ones do not block other Silicons")
    func sameKeyOrder() {
        var scheduler = SlotScheduler()
        scheduler.enqueue(entry("a1", prod, audio: true, seq: 1))
        scheduler.enqueue(entry("a2", prod, seq: 2))
        scheduler.enqueue(entry("b1", other, seq: 3))
        // a1 waits for the playing audio, so a2 (same Silicon) must wait too; b1 may go.
        #expect(scheduler.popNext(gate: SlotGate(), audioPlaying: true)?.id.raw == "b1")
        #expect(scheduler.popNext(gate: SlotGate(), audioPlaying: true) == nil)
        #expect(scheduler.popNext(gate: SlotGate(), audioPlaying: false)?.id.raw == "a1")
        // A new arrival from a Silicon with something queued joins the queue.
        #expect(scheduler.decide(entry("a3", prod, seq: 9), active: nil, gate: SlotGate(), audioPlaying: false) == .enqueue)
    }

    @Test("a summon moves the entry to the front and lets it pass the gates")
    func summonEntry() {
        var scheduler = SlotScheduler()
        scheduler.enqueue(entry("t1", test, seq: 1))
        scheduler.enqueue(entry("t2", test, seq: 2))
        let gate = SlotGate(paused: false, showTestPeeks: false)
        #expect(scheduler.popNext(gate: gate, audioPlaying: false) == nil)
        scheduler.summon(BubbleID("t2"))
        #expect(scheduler.queue.first?.id.raw == "t2")
        #expect(scheduler.popNext(gate: gate, audioPlaying: false)?.id.raw == "t2")
    }
}
