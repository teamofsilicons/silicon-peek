import Foundation
import PeekCore
import Testing

@testable import PeekUI

@Suite("Slot manager (headless surfaces)")
@MainActor
struct SlotManagerTests {
    typealias F = SlotFixtures

    @Test("a show slides in on its slot, publishes phases to the input hub, and leaves by itself")
    func showRoundTrip() async {
        let h = SlotManagerHarness()
        h.manager.updateTable([F.dj])
        h.manager.present(F.show("snd_1", durationMs: 40))
        #expect(h.manager.sends["snd_1"] != nil)
        let surface = h.surface(.bottom)
        #expect(surface?.slideIns == 1)
        #expect(h.input.states[F.dj.siliconKey]?.phase == .entering)
        #expect(h.hosts[F.dj.siliconKey]?.attached == true)
        #expect(await h.eventually { h.manager.sends.isEmpty })
        #expect(surface?.slideOuts == 1)
        #expect(h.input.states[F.dj.siliconKey]?.phase == .hidden)
        #expect(h.sentOutbounds.contains { if case .shownDone(.auto, _) = $0 { true } else { false } })
        let events = h.hosts[F.dj.siliconKey]?.events ?? []
        #expect(events.first == .enter)
        #expect(events.contains { if case .send = $0 { true } else { false } })
        #expect(events.last == .leave)
        #expect(h.manager.isIdle)
    }

    @Test("an ask queues the next send; answering it lets the queued one in")
    func queueBehindAsk() async {
        let h = SlotManagerHarness()
        h.manager.updateTable([F.dj])
        h.manager.present(F.ask("snd_ask", kind: F.keepDelete))
        h.manager.present(F.show("snd_next", durationMs: 5000))
        #expect(h.manager.queue(on: .bottom).map(\.id.raw) == ["snd_next"])
        #expect(await h.eventually { h.manager.phase(of: .bottom) == .asking })
        h.manager.chrome(on: .bottom)?.actions.option("keep")
        #expect(await h.eventually { h.manager.machine(on: .bottom)?.id.raw == "snd_next" })
        let answer = h.sent.first { if case .answer = $0.1 { true } else { false } }
        #expect(answer?.0.askID == "ask_1")
        #expect(answer?.0.sendID == "snd_ask")
        #expect(answer?.1 == .answer(.choice("keep"), via: .click))
        #expect(h.hosts[F.dj.siliconKey]?.events.contains(.answer(value: .choice("keep"), via: .click)) == true)
    }

    @Test("a new show from the same Silicon replaces the visible one")
    func replaceShow() async {
        let h = SlotManagerHarness()
        h.manager.updateTable([F.dj])
        h.manager.present(F.show("a", durationMs: 5000))
        h.manager.present(F.show("b", durationMs: 5000))
        #expect(h.manager.machine(on: .bottom)?.stage == .leaving)
        #expect(await h.eventually { h.manager.machine(on: .bottom)?.id.raw == "b" })
        #expect(h.manager.sends["a"] == nil)
    }

    @Test("production pre-empts a test bubble, which comes back afterwards with its ask still pending")
    func preemption() async {
        let h = SlotManagerHarness()
        h.manager.updateTable([F.dj, F.djTest])
        h.manager.present(F.ask("t1", askID: "ask_t", context: F.djTest.context, kind: F.keepDelete))
        #expect(await h.eventually { h.manager.phase(of: .bottom) == .asking })
        #expect(h.manager.chrome(on: .bottom)?.content.badge == .test(name: "staging"))
        h.manager.present(F.show("p1", durationMs: 300))
        #expect(await h.eventually { h.manager.machine(on: .bottom)?.id.raw == "p1" })
        #expect(h.manager.queue(on: .bottom).map(\.id.raw) == ["t1"])
        #expect(await h.eventually { h.manager.machine(on: .bottom)?.id.raw == "t1" })
        #expect(h.manager.machine(on: .bottom)?.speech == BubbleMachine.Speech.none)
        #expect(h.sentOutbounds.allSatisfy { if case .dismissed = $0 { false } else { true } },
                "pre-emption is not a dismissal")
    }

    @Test("pause and 'Show test peeks' hold bubbles back until lifted")
    func gates() async {
        let h = SlotManagerHarness()
        h.manager.updateTable([F.dj, F.djTest])
        h.manager.setGate(SlotGate(paused: true, showTestPeeks: true))
        h.manager.present(F.show("p", durationMs: 30))
        #expect(h.manager.machine(on: .bottom) == nil)
        #expect(h.manager.sends["p"] != nil, "queued sends count as visible for the menu bar and updates")
        #expect(!h.manager.isIdle)
        h.manager.setGate(SlotGate(paused: false, showTestPeeks: false))
        #expect(h.manager.machine(on: .bottom)?.id.raw == "p")
        h.manager.present(F.show("t", context: F.djTest.context, durationMs: 30))
        #expect(await h.eventually { h.manager.machine(on: .bottom) == nil })
        #expect(h.manager.queue(on: .bottom).map(\.id.raw) == ["t"])
        h.manager.setGate(SlotGate(paused: false, showTestPeeks: true))
        #expect(h.manager.machine(on: .bottom)?.id.raw == "t")
    }

    @Test("down-arrow: one click keeps the audio routed; a double click stops it and drops late chunks")
    func downArrow() async {
        let h = SlotManagerHarness()
        h.manager.updateTable([F.dj])
        h.manager.present(F.show("s", speak: "a long song intro"))
        h.manager.routeTTS(.begin(TTSBegin(sendID: "s")))
        #expect(h.speech.handled.count == 1)
        h.speech.playbacks["s"] = SpeechPlayback(sendID: "s", level: 0.5, progress: 0.1, done: false, started: true,
                                                 playedMs: 100, totalMs: nil)
        #expect(await h.eventually { h.manager.phase(of: .bottom) == .speaking })
        h.surface(.bottom)?.onDownArrowClick?(1)
        #expect(await h.eventually { h.manager.machine(on: .bottom) == nil })
        #expect(await h.eventually { h.sentOutbounds.contains(.dismissed(.downArrow)) })
        #expect(!h.manager.isIdle, "the audio plays on after the bubble left")
        h.manager.routeTTS(.chunk(TTSChunk(sendID: "s", seq: 0, pcm: Data([1, 2]))))
        #expect(h.speech.handled.count == 2, "chunks still reach the player")
        h.manager.speechFinished(SpeechFinished(sendID: "s", stoppedByUser: false, playedMs: 900, totalMs: 900))
        #expect(h.manager.isIdle)

        let d = SlotManagerHarness()
        d.manager.updateTable([F.dj])
        d.manager.present(F.show("s2", speak: "stop me"))
        d.manager.routeTTS(.begin(TTSBegin(sendID: "s2")))
        #expect(await d.eventually { d.manager.machine(on: .bottom)?.stage == .visible })
        d.surface(.bottom)?.onDownArrowClick?(1)
        d.surface(.bottom)?.onDownArrowClick?(2)
        #expect(d.speech.stopped == ["s2"])
        #expect(await d.eventually { d.sentOutbounds.contains(.dismissed(.downArrowDouble)) })
        #expect(!d.sentOutbounds.contains(.dismissed(.downArrow)))
        #expect(d.stoppedBeforeStart.map(\.0) == ["s2"], "speech.done is still reported when no audio had played")
        #expect(d.stoppedBeforeStart.first?.1 == true)
        d.manager.routeTTS(.chunk(TTSChunk(sendID: "s2", seq: 0, pcm: Data([1]))))
        #expect(d.speech.handled.count == 1, "late chunks of stopped audio are dropped")
        #expect(await d.eventually { d.manager.isIdle })
    }

    @Test("TTS for a queued send is held back and played when its bubble slides in")
    func bufferedTTS() async {
        let h = SlotManagerHarness()
        h.manager.updateTable([F.dj])
        h.manager.present(F.ask("first", kind: F.keepDelete))
        h.manager.present(F.show("second", speak: "later"))
        h.manager.routeTTS(.begin(TTSBegin(sendID: "second")))
        h.manager.routeTTS(.chunk(TTSChunk(sendID: "second", seq: 0, pcm: Data([7]))))
        #expect(h.speech.handled.isEmpty)
        h.manager.cancel(sendID: "first")
        #expect(await h.eventually { h.manager.machine(on: .bottom)?.id.raw == "second" })
        #expect(h.speech.handled.map(\.sendID) == ["second", "second"])
    }

    @Test("the hotkey on an empty slot opens a summon bubble; \\ records and Return sends a voice message")
    func summonAndVoice() async {
        let h = SlotManagerHarness()
        h.manager.updateTable([F.dj])
        h.manager.summon(.bottom)
        let machine = h.manager.machine(on: .bottom)
        #expect(machine?.isSummon == true)
        #expect(h.surface(.bottom)?.isKeyFocused == true)
        #expect(await h.eventually { h.sentOutbounds.first == .focus })
        #expect(h.manager.chrome(on: .bottom)?.content.hint != nil)
        #expect(h.manager.handleKey(KeyInput(characters: "\\", charactersIgnoringModifiers: "\\", keyCode: 0x2A), on: .bottom))
        #expect(await h.eventually { h.mic.starts == 1 })
        #expect(h.manager.phase(of: .bottom) == .listening || h.manager.phase(of: .bottom) == .entering)
        #expect(h.manager.handleKey(KeyInput(characters: "\r", charactersIgnoringModifiers: "\r", keyCode: 0x24), on: .bottom))
        #expect(await h.eventually { h.sentOutbounds.contains { if case .voice = $0 { true } else { false } } })
        let voice = h.sent.first { if case .voice = $0.1 { true } else { false } }
        #expect(voice?.0.askID == nil)
        #expect(voice?.0.sendID == nil)
        #expect(await h.eventually { h.manager.machine(on: .bottom) == nil })
    }

    @Test("a voice message that fails to transcribe comes back as a typing field with a notice")
    func failedVoiceMessageOffersTyping() async throws {
        let h = SlotManagerHarness()
        h.manager.updateTable([F.dj])
        h.manager.summon(.bottom)
        #expect(h.manager.handleKey(KeyInput(characters: "\\", charactersIgnoringModifiers: "\\", keyCode: 0x2A), on: .bottom))
        #expect(await h.eventually { h.mic.starts == 1 })
        #expect(h.manager.handleKey(KeyInput(characters: "\r", charactersIgnoringModifiers: "\r", keyCode: 0x24), on: .bottom))
        #expect(await h.eventually { h.manager.machine(on: .bottom) == nil }, "the bubble leaves once peekd accepted it")
        let messageID = try #require(h.messageIDs.last)
        h.manager.applySTT(STTResultEvent(askID: nil, messageID: messageID, outcome: .failed))
        let machine = h.manager.machine(on: .bottom)
        #expect(machine?.input == .typing)
        #expect(machine?.notice == BubbleMachine.transcribeFailedNotice)
        #expect(h.surface(.bottom)?.isKeyFocused == true)
        // A later outcome for a message nobody is waiting for changes nothing.
        h.manager.applySTT(STTResultEvent(askID: nil, messageID: "cmsg_unknown", outcome: .failed))
        #expect(h.manager.machine(on: .bottom)?.input == .typing)
    }

    /// Summons `slot`, records with `\` and sends with Return; waits until peekd "accepted" it and the bubble left.
    private func sendVoiceMessage(_ h: SlotManagerHarness, on slot: SlotIndex) async -> Bool {
        let starts = h.mic.starts
        h.manager.summon(slot)
        guard h.manager.handleKey(KeyInput(characters: "\\", charactersIgnoringModifiers: "\\", keyCode: 0x2A), on: slot)
        else { return false }
        guard await h.eventually(.seconds(3), { h.mic.starts == starts + 1 }) else { return false }
        guard h.manager.handleKey(KeyInput(characters: "\r", charactersIgnoringModifiers: "\r", keyCode: 0x24), on: slot)
        else { return false }
        return await h.eventually { h.manager.machine(on: slot) == nil }
    }

    @Test("voice-message outcomes are matched by the message id peekd named, not by arrival order")
    func voiceMessagesMatchByID() async throws {
        let h = SlotManagerHarness()
        h.manager.updateTable([F.dj, F.cleanup])
        #expect(await sendVoiceMessage(h, on: .bottom))
        #expect(await sendVoiceMessage(h, on: .right))
        #expect(h.messageIDs.count == 2)
        let (djMessage, cleanupMessage) = (h.messageIDs[0], h.messageIDs[1])
        // The second message's outcome arrives first: it belongs to Cleanup's slot, not to DJ's.
        h.manager.applySTT(STTResultEvent(askID: nil, messageID: cleanupMessage, outcome: .failed))
        #expect(h.manager.machine(on: .right)?.input == .typing)
        #expect(h.manager.machine(on: .bottom) == nil)
        // DJ's message was transcribed and delivered: nothing to offer.
        h.manager.applySTT(STTResultEvent(askID: nil, messageID: djMessage, outcome: .matched, value: "hello"))
        #expect(h.manager.machine(on: .bottom) == nil)
    }

    @Test("an outcome that overtakes the voice.submit reply is applied once the reply names its message")
    func earlyVoiceOutcome() async throws {
        let h = SlotManagerHarness()
        h.manager.updateTable([F.dj])
        h.beforeReply = { [unowned h] outbound, messageID in
            guard case .voice = outbound, let messageID else { return }
            h.manager.applySTT(STTResultEvent(askID: nil, messageID: messageID, outcome: .failed))
        }
        h.manager.summon(.bottom)
        #expect(h.manager.handleKey(KeyInput(characters: "\\", charactersIgnoringModifiers: "\\", keyCode: 0x2A), on: .bottom))
        #expect(await h.eventually { h.mic.starts == 1 })
        #expect(h.manager.handleKey(KeyInput(characters: "\r", charactersIgnoringModifiers: "\r", keyCode: 0x24), on: .bottom))
        // The sending bubble slides out, and a typing field follows it on the same slot.
        #expect(await h.eventually {
            h.manager.machine(on: .bottom)?.input == .typing
                && h.manager.machine(on: .bottom)?.notice == BubbleMachine.transcribeFailedNotice
        })
        #expect(h.messageIDs.count == 1)
    }

    @Test("an older peekd names no message: outcomes are matched in order")
    func unnamedVoiceMessagesMatchInOrder() async throws {
        let h = SlotManagerHarness()
        h.namesVoiceMessages = false
        h.manager.updateTable([F.dj])
        #expect(await sendVoiceMessage(h, on: .bottom))
        #expect(h.messageIDs.isEmpty)
        h.manager.applySTT(STTResultEvent(askID: nil, messageID: "cmsg_from_old_peekd", outcome: .empty))
        #expect(h.manager.machine(on: .bottom)?.input == .typing)
    }

    @Test("a printable key after the hotkey starts typing seeded with it; Esc slides back")
    func summonTyping() async {
        let h = SlotManagerHarness()
        h.manager.updateTable([F.dj])
        h.manager.present(F.show("s", durationMs: 5000))
        h.manager.summon(.bottom)
        #expect(h.manager.handleKey(KeyInput(characters: "y", charactersIgnoringModifiers: "y", keyCode: 16), on: .bottom))
        #expect(h.manager.machine(on: .bottom)?.typingText == "y")
        #expect(h.manager.chrome(on: .bottom)?.typingText == "y")
        #expect(h.manager.chrome(on: .bottom)?.content.input == .typing)
        #expect(h.input.states[F.dj.siliconKey]?.typingText == "y")
        #expect(h.manager.handleKey(KeyInput(characters: nil, charactersIgnoringModifiers: nil, keyCode: 0x35), on: .bottom))
        #expect(await h.eventually { h.sentOutbounds.contains(.dismissed(.esc)) })
        #expect(await h.eventually { h.manager.machine(on: .bottom) == nil })
    }

    @Test("arrow keys move the highlight and Return answers with via keyboard")
    func keyboardChoice() async {
        let h = SlotManagerHarness()
        h.manager.updateTable([F.dj])
        h.manager.present(F.ask("s", kind: F.keepDelete))
        h.manager.summon(.bottom)
        let right = KeyInput(characters: "\u{F703}", charactersIgnoringModifiers: "\u{F703}", keyCode: KeyClassifier.rightArrow)
        h.manager.handleKey(right, on: .bottom)
        h.manager.handleKey(right, on: .bottom)
        #expect(h.manager.machine(on: .bottom)?.highlight == "delete")
        #expect(h.manager.chrome(on: .bottom)?.highlight == "delete")
        h.manager.handleKey(KeyInput(characters: "\r", charactersIgnoringModifiers: "\r", keyCode: KeyClassifier.returnKey),
                            on: .bottom)
        #expect(await h.eventually { h.sentOutbounds.contains(.answer(.choice("delete"), via: .keyboard)) })
    }

    @Test("stt.result routes by ask id; a refused upload shows peekd's reason")
    func sttAndFailures() async {
        let h = SlotManagerHarness()
        h.manager.updateTable([F.dj])
        h.manager.present(F.ask("s", askID: "ask_9", kind: F.keepDelete))
        #expect(await h.eventually { h.manager.phase(of: .bottom) == .asking })
        h.manager.chrome(on: .bottom)?.actions.mic()
        #expect(await h.eventually { h.mic.starts == 1 })
        h.failNext = "Not sent: Peek's helper (peekd) isn't reachable. Try again in a moment."
        h.manager.chrome(on: .bottom)?.actions.mic()
        #expect(await h.eventually { h.manager.machine(on: .bottom)?.notice?.contains("isn't reachable") == true })
        #expect(await h.eventually { h.manager.phase(of: .bottom) == .asking })
        h.manager.applySTT(STTResultEvent(askID: "ask_other", messageID: nil, outcome: .matched, value: "keep"))
        #expect(h.manager.machine(on: .bottom) != nil, "another ask's result changes nothing")
        h.manager.applySTT(STTResultEvent(askID: "ask_9", messageID: nil, outcome: .matched, value: "keep"))
        #expect(await h.eventually { h.manager.machine(on: .bottom) == nil })
    }

    @Test("a Silicon moving slots takes its queued and visible bubbles along")
    func relocation() async {
        let h = SlotManagerHarness()
        h.manager.updateTable([F.dj])
        h.manager.present(F.ask("s", kind: F.keepDelete))
        #expect(await h.eventually { h.manager.phase(of: .bottom) == .asking })
        let moved = SlotState(index: .left, actorID: "si:dj", orgID: "tos", displayName: "DJ")
        h.manager.updateTable([moved])
        #expect(await h.eventually { h.manager.machine(on: .left)?.id.raw == "s" })
        #expect(h.manager.machine(on: .bottom) == nil)
        #expect(h.hosts[F.dj.siliconKey]?.events.contains(.move(from: .bottom, to: .left)) == true)
    }

    @Test("dismissAll hides everything without telling peekd")
    func dismissAll() async {
        let h = SlotManagerHarness()
        h.manager.updateTable([F.dj])
        h.manager.present(F.ask("a", kind: F.keepDelete))
        h.manager.present(F.show("b"))
        h.manager.dismissAll()
        #expect(await h.eventually { h.manager.isIdle })
        #expect(h.sentOutbounds.isEmpty)
        #expect(h.manager.sends.isEmpty)
    }

    @Test("mode changes re-place idle panels at once")
    func modeChange() {
        let h = SlotManagerHarness()
        h.manager.setMode(.compact)
        #expect(h.manager.chrome(on: .top)?.slotLayout.mode == .compact)
        #expect(h.manager.chrome(on: .top)?.layout.stripRect != nil)
    }
}
