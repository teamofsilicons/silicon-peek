import Foundation
import PeekCore
import Testing

@testable import PeekUI

/// peek 0.1.2 slot-manager behaviour (contract §8.2–§8.7): the pre-warm, `shown`, the "+N" badge, `--replace`, early
/// backdrop sampling and the Esc router.
@Suite("Slot manager 0.1.2: pre-warm, shown, badge, replace, backdrop, Esc router")
@MainActor
struct SlotManager012Tests {
    typealias F = SlotFixtures

    private func shownCount(_ h: SlotManagerHarness, _ sendID: String) -> Int {
        h.sent.filter { $0.1 == .shown && $0.0.sendID == sendID }.count
    }

    // MARK: Pre-warm

    @Test("the pre-warm waits for the drawing's first frame; the 0.6 s cap slides in without it")
    func prewarmWaitsForFrame() async {
        let h = SlotManagerHarness()
        h.manager.updateTable([F.dj])
        let host = SlotFakeHost(key: F.dj.siliconKey)
        host.framesArrive = false
        h.hosts[F.dj.siliconKey] = host
        h.manager.present(F.show("s", durationMs: 5000))
        try? await Task.sleep(for: .milliseconds(80))
        #expect(h.manager.machine(on: .bottom)?.stage == .prewarming)
        #expect(h.surface(.bottom)?.isPrewarming == true)
        #expect(h.surface(.bottom)?.slideIns == 0)
        #expect(host.frameRequests == 1)
        #expect(await h.eventually { h.surface(.bottom)?.slideIns == 1 }, "the cap (prewarmMax) slides it in anyway")
        #expect(h.manager.machine(on: .bottom)?.stage == .entering || h.manager.machine(on: .bottom)?.stage == .visible)
    }

    @Test("the pre-warm waits for a backdrop sample: fresh with the screen source, any with the wallpaper, or backdropWait")
    func prewarmWaitsForBackdrop() async {
        var timing = SlotManagerHarness.fastTiming
        timing.backdropWait = 0.2
        timing.prewarmMax = 5
        let h = SlotManagerHarness(timing: timing)
        h.manager.updateTable([F.dj])
        h.backdrop.ages = [:]  // never sampled
        h.manager.present(F.show("s", durationMs: 5000))
        try? await Task.sleep(for: .milliseconds(120))
        #expect(h.surface(.bottom)?.slideIns == 0, "no sample yet and backdropWait not over")
        h.clock = 0.25
        #expect(await h.eventually { h.surface(.bottom)?.slideIns == 1 }, "backdropWait passed (checked on a frame tick)")

        let screen = SlotManagerHarness(timing: timing)
        screen.manager.updateTable([F.dj])
        screen.backdrop.source = .screen
        screen.backdrop.ages = [F.dj.siliconKey: 5]  // stale for the screen source
        screen.manager.present(F.show("t", durationMs: 5000))
        try? await Task.sleep(for: .milliseconds(120))
        #expect(screen.surface(.bottom)?.slideIns == 0)
        screen.backdrop.ages = [F.dj.siliconKey: 0.4]
        #expect(await screen.eventually { screen.surface(.bottom)?.slideIns == 1 })
    }

    @Test("a surface that does not pre-warm (PEEK_DEBUG_PREWARM=none) slides in at once, without waiting for the frame")
    func noPrewarmSurface() async {
        let h = SlotManagerHarness()
        h.manager.updateTable([F.dj])
        h.manager.setMode(.normal)
        let host = SlotFakeHost(key: F.dj.siliconKey)
        host.framesArrive = false
        h.hosts[F.dj.siliconKey] = host
        _ = h.manager.chrome(on: .bottom)
        h.manager.present(F.show("warm", durationMs: 5000))
        #expect(await h.eventually { h.surface(.bottom)?.slideIns == 1 })
        h.manager.cancel(sendID: "warm")
        #expect(await h.eventually { h.manager.machine(on: .bottom) == nil })
        h.surface(.bottom)?.skipsPrewarm = true
        h.manager.present(F.show("cold", durationMs: 5000))
        #expect(await h.eventually(.milliseconds(150), { h.surface(.bottom)?.slideIns == 2 }), "no 0.3 s cap wait")
        #expect(h.surface(.bottom)?.prewarms == 1, "the second bubble never pre-warmed")
    }

    @Test("a bubble cancelled while it pre-warms is ordered out unseen and the next one follows")
    func cancelWhilePrewarming() async {
        var timing = SlotManagerHarness.fastTiming
        timing.prewarmMax = 5
        let h = SlotManagerHarness(timing: timing)
        h.manager.updateTable([F.dj])
        let host = SlotFakeHost(key: F.dj.siliconKey)
        host.framesArrive = false
        h.hosts[F.dj.siliconKey] = host
        h.manager.present(F.show("a", durationMs: 5000))
        #expect(h.surface(.bottom)?.isPrewarming == true)
        h.manager.cancel(sendID: "a")
        #expect(h.manager.machine(on: .bottom) == nil)
        #expect(h.surface(.bottom)?.isPrewarming == false)
        #expect(h.surface(.bottom)?.slideOuts == 0, "never on screen, so no slide-out")
        #expect(h.manager.sends["a"] == nil)
    }

    // MARK: shown

    @Test("shown goes to peekd once per send as it pre-warms: never for summons, not while paused, again after the gate opens")
    func shownOnce() async {
        let h = SlotManagerHarness()
        h.manager.updateTable([F.dj, F.djTest])
        h.manager.setGate(SlotGate(paused: true, showTestPeeks: true))
        h.manager.present(F.show("p", durationMs: 40))
        try? await Task.sleep(for: .milliseconds(30))
        #expect(shownCount(h, "p") == 0, "held back while paused: not shown")
        h.manager.setGate(SlotGate())
        #expect(await h.eventually { shownCount(h, "p") == 1 })
        #expect(await h.eventually { h.manager.machine(on: .bottom) == nil })

        // A pre-empted test bubble that comes back does not report shown again.
        h.manager.present(F.ask("t1", askID: "ask_t", context: F.djTest.context, kind: F.keepDelete))
        #expect(await h.eventually { h.manager.phase(of: .bottom) == .asking })
        h.manager.present(F.show("p2", durationMs: 60))
        #expect(await h.eventually { h.manager.machine(on: .bottom)?.id.raw == "p2" })
        #expect(await h.eventually { h.manager.machine(on: .bottom)?.id.raw == "t1" })
        #expect(await h.eventually { h.manager.machine(on: .bottom)?.stage == .visible })
        #expect(shownCount(h, "t1") == 1)
        #expect(shownCount(h, "p2") == 1)

        h.manager.summon(.right)
        h.manager.updateTable([F.dj, F.djTest, F.cleanup])
        h.manager.summon(.right)
        #expect(await h.eventually { h.manager.machine(on: .right)?.stage == .visible || h.manager.machine(on: .right)?.stage == .entering })
        #expect(!h.sent.contains { $0.1 == .shown && $0.0.slot == .right }, "summons are not sends")
    }

    // MARK: Badge

    @Test("the +N badge starts at peek.show's queued_behind and follows queue.state for that send only")
    func waitingBadge() async {
        let h = SlotManagerHarness()
        h.manager.updateTable([F.dj])
        var event = F.show("a", durationMs: 5000)
        event.queuedBehind = 2
        h.manager.present(event)
        let chrome = h.manager.chrome(on: .bottom)
        #expect(chrome?.content.waiting == 2)
        #expect(chrome?.layout.buttons.badge != nil)
        h.manager.applyQueueState(QueueStateEvent(slot: .bottom, sendID: "snd_other", waiting: 5))
        #expect(chrome?.content.waiting == 2, "another send's count is ignored")
        h.manager.applyQueueState(QueueStateEvent(slot: .bottom, context: .testing(environmentID: "0192-env"), sendID: "a", waiting: 5))
        #expect(chrome?.content.waiting == 2, "another context's count is ignored")
        h.manager.applyQueueState(QueueStateEvent(slot: .bottom, sendID: "a", waiting: 4))
        #expect(chrome?.content.waiting == 4)
        h.manager.applyQueueState(QueueStateEvent(slot: .bottom, sendID: "a", waiting: 0))
        #expect(chrome?.content.waiting == 0)
        #expect(chrome?.layout.buttons.badge == nil, "0 hides the badge")
    }

    @Test("a queue.state for a send still waiting for its slot is applied when it activates")
    func waitingBadgeForPendingSend() async {
        let h = SlotManagerHarness()
        h.manager.updateTable([F.dj, F.djTest])
        h.manager.present(F.ask("p", kind: F.keepDelete))  // production holds the slot
        var test = F.show("t", context: F.djTest.context, durationMs: 5000)
        test.queuedBehind = 1
        h.manager.present(test)
        #expect(h.manager.queue(on: .bottom).map(\.id.raw) == ["t"])
        h.manager.applyQueueState(QueueStateEvent(slot: .bottom, context: F.djTest.context, sendID: "t", waiting: 3))
        h.manager.cancel(sendID: "p")
        #expect(await h.eventually { h.manager.machine(on: .bottom)?.id.raw == "t" })
        #expect(h.manager.chrome(on: .bottom)?.content.waiting == 3)
    }

    // MARK: --replace

    @Test("--replace: peek.cancel(replaced) takes the old bubble away with its speech; the new peek.show follows at once")
    func replaceFlow() async {
        let h = SlotManagerHarness()
        h.manager.updateTable([F.dj])
        h.manager.present(F.show("old", speak: "the first version", texts: ["v1"]))
        h.manager.routeTTS(.begin(TTSBegin(sendID: "old")))
        h.speech.playbacks["old"] = SpeechPlayback(sendID: "old", level: 0.3, progress: 0.2, done: false, started: true,
                                                   playedMs: 200, totalMs: nil)
        #expect(await h.eventually { h.manager.machine(on: .bottom)?.stage == .visible })
        h.manager.cancel(sendID: "old")
        var replacement = F.show("new", texts: ["v2"], durationMs: 5000)
        replacement.replaces = "old"
        h.manager.present(replacement)
        #expect(h.speech.stopped.contains("old"), "the replaced bubble's speech stops")
        #expect(h.manager.machine(on: .bottom)?.stage == .leaving)
        #expect(h.manager.queue(on: .bottom).map(\.id.raw) == ["new"])
        #expect(await h.eventually { h.manager.machine(on: .bottom)?.id.raw == "new" })
        #expect(!h.sentWithoutShown.contains { if case .dismissed = $0 { true } else { false } }, "no dismissal is reported")
    }

    // MARK: Backdrop

    @Test("the backdrop is sampled when a send arrives (before the slide), and occupied slots are kept warm")
    func earlyBackdrop() async {
        let h = SlotManagerHarness()
        h.manager.updateTable([F.dj, F.cleanup])
        let djRect = F.layout(.bottom).visualFrameOnScreen
        #expect(h.backdrop.warmed[F.dj.siliconKey] == djRect)
        #expect(h.backdrop.warmed[F.cleanup.siliconKey] == F.layout(.right).visualFrameOnScreen)
        let host = SlotFakeHost(key: F.dj.siliconKey)
        host.framesArrive = false
        h.hosts[F.dj.siliconKey] = host
        h.manager.present(F.show("s", durationMs: 5000))
        #expect(h.backdrop.trackCalls.first.map { $0.0 == F.dj.siliconKey && $0.1 == djRect } == true)
        #expect(h.surface(.bottom)?.slideIns == 0, "sampled before the bubble moved")
        h.manager.updateTable([F.dj])
        #expect(h.backdrop.warmed[F.cleanup.siliconKey] == nil, "an emptied slot is no longer warmed")

        // A queued send withdrawn before it showed stops its arrival sampling.
        let q = SlotManagerHarness()
        q.manager.updateTable([F.dj, F.djTest])
        q.manager.present(F.ask("p", kind: F.keepDelete))
        q.manager.present(F.show("t", context: F.djTest.context, durationMs: 5000))
        #expect(q.backdrop.tracked[F.djTest.siliconKey] != nil)
        q.manager.cancel(sendID: "t")
        #expect(q.backdrop.tracked[F.djTest.siliconKey] == nil)
        #expect(q.backdrop.tracked[F.dj.siliconKey] != nil, "the bubble on screen keeps its own")
    }

    // MARK: Esc router

    @Test("Esc is held for exactly 3 s from each slide-in (the next queued bubble gets its own window), never later")
    func graceWindow() async {
        let h = SlotManagerHarness()
        h.manager.updateTable([F.dj])
        h.manager.present(F.show("a", durationMs: 5000))
        #expect(!h.escapeKey.isHeld, "not while it pre-warms (nobody sees it yet)")
        #expect(await h.eventually { h.manager.machine(on: .bottom)?.graceUntil != nil })
        #expect(h.manager.machine(on: .bottom)?.graceUntil == 3)
        #expect(h.escapeKey.isHeld)
        h.clock = 2.999
        h.manager.refreshEscape()
        #expect(h.escapeKey.isHeld)
        h.clock = 3.0
        h.manager.refreshEscape()
        #expect(!h.escapeKey.isHeld, "released at 3.0 s exactly")

        // The next queued bubble of the same Silicon opens its own window when it slides in.
        h.manager.present(F.show("b", durationMs: 5000))
        h.manager.cancel(sendID: "a")
        h.clock = 10
        #expect(await h.eventually { h.manager.machine(on: .bottom)?.id.raw == "b" && h.manager.machine(on: .bottom)?.graceUntil != nil })
        #expect(h.manager.machine(on: .bottom)?.graceUntil == 13)
        #expect(h.escapeKey.isHeld)
        h.keyWindowOpen = true
        h.manager.refreshEscape()
        #expect(!h.escapeKey.isHeld, "a key Peek window gets Esc through AppKit")
        h.keyWindowOpen = false
        h.manager.refreshEscape()
        #expect(h.escapeKey.isHeld)
    }

    @Test("hover holds Esc after the grace; a press goes to the hovered bubble; with nothing to aim at Esc is released")
    func hoverEsc() async {
        let h = SlotManagerHarness()
        h.manager.updateTable([F.dj])
        h.manager.present(F.show("a", speak: "hello there", texts: ["hi"], durationMs: 5000))
        #expect(await h.eventually { h.manager.machine(on: .bottom)?.stage == .visible })
        h.clock = 4
        h.manager.refreshEscape()
        #expect(!h.escapeKey.isHeld)
        h.surface(.bottom)?.pointerOverContent = true
        h.manager.refreshEscape()
        #expect(h.escapeKey.isHeld)
        #expect(h.escapeKey.press())
        #expect(h.manager.machine(on: .bottom)?.leaveReason == .escaped)
        #expect(h.manager.machine(on: .bottom)?.escArmedUntil == 4.08)
        #expect(!h.speech.stopped.contains("a"), "one Esc keeps the audio")
        h.surface(.bottom)?.pointerOverContent = false
        #expect(h.escapeKey.press(), "the double-Esc window keeps Esc for this bubble")
        #expect(h.speech.stopped.contains("a"))
        #expect(await h.eventually { h.sentOutbounds.contains(.dismissed(.escDouble)) })
        #expect(await h.eventually { h.manager.machine(on: .bottom) == nil })
        h.manager.refreshEscape()
        #expect(!h.escapeKey.isHeld)
    }

    @Test("targets: an open popup first (it only closes), then the latest Esc window, then hover, then the newest bubble")
    func targetPrecedence() async throws {
        let h = SlotManagerHarness()
        h.manager.updateTable([F.dj, F.cleanup])
        let long = String(repeating: "The nightly export finished with twelve warnings about stale caches. ", count: 8)
        h.manager.present(F.show("a", slot: .bottom, texts: [long], durationMs: 60_000))
        #expect(await h.eventually { h.manager.machine(on: .bottom)?.stage == .visible })
        h.clock = 1
        h.manager.present(F.ask("b", slot: .right, kind: F.keepDelete))
        #expect(await h.eventually { h.manager.machine(on: .right)?.stage == .visible })

        // (4) The newest bubble: b (an ask) collapses.
        #expect(h.escapeKey.press())
        #expect(h.manager.machine(on: .right)?.isAskCollapsed == true)
        #expect(h.manager.machine(on: .bottom)?.stage == .visible)

        // (1) A popup beats everything and only closes.
        let chrome = try #require(h.manager.chrome(on: .bottom))
        let target = ChromeTarget.item("text-0")
        try #require(chrome.layout.isExpandable(target))
        chrome.toggleExpanded(target)
        #expect(h.escapeKey.isHeld)
        #expect(h.escapeKey.press())
        #expect(chrome.expanded == nil)
        #expect(h.manager.machine(on: .right)?.stage == .visible, "the armed ask was not touched")

        // (2) b's Esc window (still open) beats hover on a.
        h.surface(.bottom)?.pointerOverContent = true
        #expect(h.escapeKey.press())
        #expect(h.manager.machine(on: .right)?.stage == .leaving, "the second Esc dismissed the compact ask")
        #expect(await h.eventually { h.sentOutbounds.contains(.dismissed(.esc)) })

        // (3) Then hover: a.
        h.clock = 30
        #expect(await h.eventually { h.manager.machine(on: .right) == nil })
        h.manager.refreshEscape()
        #expect(h.escapeKey.press())
        #expect(h.manager.machine(on: .bottom)?.leaveReason == .escaped)
    }

    @Test("an Esc typed into a key panel goes to its own bubble unless another bubble's Esc window is open")
    func keyPanelEsc() async {
        let h = SlotManagerHarness()
        h.manager.updateTable([F.dj])
        h.manager.present(F.ask("s", kind: .text(placeholder: nil, maxLength: 60)))
        h.manager.summon(.bottom)
        #expect(await h.eventually { h.surface(.bottom)?.isKeyFocused == true })
        #expect(h.manager.handleKey(KeyInput(characters: "y", charactersIgnoringModifiers: "y", keyCode: 16), on: .bottom))
        #expect(h.manager.machine(on: .bottom)?.input == .typing)
        #expect(h.manager.handleKey(KeyInput(characters: nil, charactersIgnoringModifiers: nil, keyCode: KeyClassifier.escape),
                                    on: .bottom))
        #expect(h.manager.machine(on: .bottom)?.isAskCollapsed == true)
        #expect(h.manager.machine(on: .bottom)?.typingText.isEmpty == true)
        #expect(!h.sentOutbounds.contains { if case .answer = $0 { true } else { false } }, "nothing typed is uploaded")
        #expect(h.surface(.bottom)?.isKeyFocused == false)
    }

    @Test("bare Esc taken by another app: the problem is reported and peeks keep working with the down-arrow")
    func escapeRegistrationFailure() async {
        let h = SlotManagerHarness()
        var reported: [String?] = []
        h.manager.onEscapeProblemChanged = { reported.append($0) }
        h.escapeKey.failWith = "Esc for peeks is taken by another app; use the down-arrow"
        h.manager.updateTable([F.dj])
        h.manager.present(F.show("a", durationMs: 5000))
        #expect(await h.eventually { h.manager.escapeProblem != nil })
        #expect(h.manager.escapeProblem == "Esc for peeks is taken by another app; use the down-arrow")
        #expect(reported == ["Esc for peeks is taken by another app; use the down-arrow"])
        h.surface(.bottom)?.onDownArrowClick?(1)
        #expect(await h.eventually { h.manager.machine(on: .bottom) == nil })
    }
}
