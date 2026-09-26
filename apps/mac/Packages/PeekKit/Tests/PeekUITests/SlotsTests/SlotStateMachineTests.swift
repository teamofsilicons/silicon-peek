import Foundation
import PeekCore
import Testing

@testable import PeekUI

@Suite("Slot state machine")
struct SlotStateMachineTests {
    typealias F = SlotFixtures

    // MARK: Phases and slide timing

    @Test("a show slides in, shows, auto-dismisses after its duration and reports shown.done(auto)")
    func showLifecycle() {
        let event = F.show("snd_1", texts: ["Now playing"], durationMs: 4000)
        var machine = BubbleMachine(id: .send("snd_1"), source: .send(event))
        #expect(machine.phase == .hidden)
        let begin = machine.handle(.begin, now: 10)
        #expect(begin.contains(.slideIn))
        #expect(begin.contains(.drawing(.enter)))
        #expect(begin.schedules(.enter) == 0.5)
        #expect(machine.phase == .entering)

        let entered = machine.handle(.timerFired(.enter), now: 10.5)
        #expect(machine.phase == .showing)
        #expect(entered.schedules(.autoDismiss) == 4)

        let leaving = machine.handle(.timerFired(.autoDismiss), now: 14.5)
        #expect(leaving.sent == [.shownDone(.auto, visibleMs: 4500)])
        #expect(leaving.contains(.slideOut))
        #expect(leaving.contains(.drawing(.leave)))
        #expect(machine.phase == .leaving)
        let done = machine.handle(.timerFired(.leave), now: 15)
        #expect(done.finishedRequeue == false)
        #expect(machine.phase == .hidden)
    }

    @Test("the pointer over the chrome (or an open popup) pauses a show's auto-dismiss; leaving gives it at least 2.5 s")
    func hoverPausesAutoDismiss() {
        var machine = BubbleMachine(id: .send("snd_1"), source: .send(F.show("snd_1", texts: ["Now playing"], durationMs: 4000)))
        _ = machine.handle(.begin, now: 0)
        #expect(machine.handle(.timerFired(.enter), now: 0.5).schedules(.autoDismiss) == 4)
        let entered = machine.handle(.pointer(inside: true), now: 1)
        #expect(entered.contains(.cancel(.autoDismiss)))
        #expect(machine.handle(.timerFired(.autoDismiss), now: 4.5).isEmpty, "a stale timer does nothing while hovered")
        #expect(machine.phase == .showing)
        // 3.5 s were left when the pointer came in, but 20 s passed: the show still stays 2.5 s after the pointer leaves.
        #expect(machine.handle(.pointer(inside: false), now: 21).schedules(.autoDismiss) == 2.5)
        _ = machine.handle(.pointer(inside: true), now: 21.5)
        #expect(machine.handle(.pointer(inside: false), now: 21.6).schedules(.autoDismiss) == 2.5)

        // Hovered before the enter timer fired: the auto-dismiss is deferred, then armed with what is left.
        var early = BubbleMachine(id: .send("snd_2"), source: .send(F.show("snd_2", texts: ["Hi"], durationMs: 6000)))
        _ = early.handle(.begin, now: 0)
        _ = early.handle(.pointer(inside: true), now: 0.2)
        #expect(early.handle(.timerFired(.enter), now: 0.5).schedules(.autoDismiss) == nil)
        let left = early.handle(.pointer(inside: false), now: 1.5)
        #expect(abs((left.schedules(.autoDismiss) ?? 0) - 5) < 1e-9)

        // Asks never auto-dismiss, hovered or not.
        var ask = BubbleMachine(id: .send("snd_3"), source: .send(F.ask("snd_3", askID: "ask_3", kind: .text(placeholder: nil, maxLength: 60))))
        _ = ask.handle(.begin, now: 0)
        _ = ask.handle(.timerFired(.enter), now: 0.5)
        _ = ask.handle(.pointer(inside: true), now: 1)
        #expect(ask.handle(.pointer(inside: false), now: 2).schedules(.autoDismiss) == nil)
    }

    @Test("without --duration a show uses clamp(3 + 0.06 × chars, 4, 15) s")
    func defaultShowDuration() {
        var machine = BubbleMachine(id: .send("snd_1"), source: .send(F.show("snd_1", texts: [String(repeating: "a", count: 100)])))
        _ = machine.handle(.begin, now: 0)
        let effects = machine.handle(.timerFired(.enter), now: 0.5)
        #expect(effects.schedules(.autoDismiss) == 9)
    }

    @Test("speak + show: speaking while audio plays, slides back 1.5 s after speech is done with reason speech_done")
    func speechThenLinger() {
        var machine = BubbleMachine(id: .send("s"), source: .send(F.show("s", speak: "hello there")))
        #expect(machine.speech == .waiting)
        let begin = machine.handle(.begin, now: 0)
        #expect(begin.schedules(.speechStart) == 8)
        let entered = machine.handle(.timerFired(.enter), now: 0.5)
        #expect(entered.schedules(.autoDismiss) == nil, "no auto-dismiss while speech is expected")
        _ = machine.handle(.speechStarted, now: 0.7)
        #expect(machine.phase == .speaking)
        let finished = machine.handle(.speechFinished(stoppedByUser: false), now: 3)
        #expect(finished.schedules(.autoDismiss) == 1.5)
        #expect(machine.phase == .showing)
        let leaving = machine.handle(.timerFired(.autoDismiss), now: 4.5)
        #expect(leaving.sent == [.shownDone(.speechDone, visibleMs: 4500)])
    }

    @Test("--duration after speech replaces the 1.5 s linger")
    func durationAfterSpeech() {
        var machine = F.visibleMachine(F.show("s", speak: "hi", durationMs: 2500))
        _ = machine.handle(.speechStarted, now: 1)
        let effects = machine.handle(.speechFinished(stoppedByUser: false), now: 2)
        #expect(effects.schedules(.autoDismiss) == 2.5)
    }

    @Test("TTS failure before audio: the speak text becomes a pill, then the bubble auto-dismisses")
    func speechFailureShowsPill() {
        let event = F.show("s", speak: "the quick brown fox", texts: [])
        var machine = F.visibleMachine(event)
        #expect(!machine.speakAsPill)
        let effects = machine.handle(.speechFailed, now: 1)
        #expect(machine.speakAsPill)
        // clamp(3 + 0.06 × 19 characters, 4, 15) s
        #expect(abs((effects.schedules(.autoDismiss) ?? 0) - 4.14) < 1e-9)
    }

    @Test("unsupported language: no audio expected, the speak text shows as a pill at once")
    func unsupportedLanguage() {
        let event = F.show("s", speak: "Bonjour", audio: false, texts: [])
        let machine = BubbleMachine(id: .send("s"), source: .send(event))
        #expect(machine.speech == .none)
        #expect(machine.speakAsPill)
    }

    @Test("no audio 8 s after appearing counts as a TTS failure and late audio is dropped")
    func speechStartTimeout() {
        var machine = F.visibleMachine(F.show("s", speak: "hi", texts: []))
        let effects = machine.handle(.timerFired(.speechStart), now: 8)
        #expect(effects.contains(.stopSpeech))
        #expect(machine.speech == .failed)
        #expect(machine.speakAsPill)
    }

    @Test("a pre-empted send shown again expects no audio")
    func requeuedSendHasNoSpeech() {
        let machine = BubbleMachine(id: .send("s"), source: .send(F.show("s", speak: "hi")), speechAvailable: false)
        #expect(machine.speech == .none)
        #expect(machine.phase == .hidden)
    }

    // MARK: Down-arrow

    @Test("single down-arrow click slides out, keeps the audio, and reports down_arrow after the double-click window")
    func downArrowSingle() {
        var machine = F.visibleMachine(F.show("s", speak: "long speech"))
        _ = machine.handle(.speechStarted, now: 1)
        let click = machine.handle(.downArrowClick, now: 2)
        #expect(click.contains(.slideOut))
        #expect(!click.contains(.stopSpeech))
        #expect(click.sent.isEmpty, "the gesture is reported once the double-click window closes")
        #expect(click.schedules(.doubleClick) == 0.5)
        #expect(machine.audioOutlivesBubble)
        // The slide finishes before the window closes: not finished yet.
        #expect(machine.handle(.timerFired(.leave), now: 2.45).finishedRequeue == nil)
        let window = machine.handle(.timerFired(.doubleClick), now: 2.5)
        #expect(window.sent == [.dismissed(.downArrow)])
        #expect(window.finishedRequeue == false)
    }

    @Test("double down-arrow click also stops the audio and reports down_arrow_double once")
    func downArrowDouble() {
        var machine = F.visibleMachine(F.show("s", speak: "long speech"))
        _ = machine.handle(.speechStarted, now: 1)
        _ = machine.handle(.downArrowClick, now: 2)
        let double = machine.handle(.downArrowDoubleClick, now: 2.2)
        #expect(double.contains(.stopSpeech))
        #expect(double.sent == [.dismissed(.downArrowDouble)])
        #expect(machine.speech == .stopped)
        #expect(!machine.audioOutlivesBubble)
        #expect(machine.handle(.timerFired(.doubleClick), now: 2.5).sent.isEmpty)
        #expect(machine.handle(.timerFired(.leave), now: 2.6).finishedRequeue == false)
    }

    @Test("the down-arrow while recording also turns the mic off")
    func downArrowWhileRecording() {
        var machine = F.visibleMachine(F.show("s"))
        _ = machine.handle(.micButton, now: 1)
        let effects = machine.handle(.downArrowClick, now: 2)
        #expect(effects.contains(.cancelRecording))
        #expect(machine.input == .none)
    }

    @Test("a summoned bubble with no send just slides out on the down-arrow")
    func downArrowOnSummon() {
        var machine = BubbleMachine(id: .summon(.top, serial: 1), source: .summon(.top))
        _ = machine.handle(.begin, now: 0)
        let effects = machine.handle(.downArrowClick, now: 1)
        #expect(effects.contains(.slideOut))
        #expect(effects.sent.isEmpty)
        #expect(effects.schedules(.doubleClick) == nil)
    }

    // MARK: Esc, typing, messages

    @Test("Esc while typing cancels, slides back and reports dismissed(esc)")
    func escapeWhileTyping() {
        var machine = F.visibleMachine(F.show("s"))
        _ = machine.handle(.startTyping(seed: "h"), now: 1)
        #expect(machine.phase == .typing)
        #expect(machine.typingText == "h")
        let effects = machine.handle(.escape, now: 2)
        #expect(effects.sent == [.dismissed(.esc)])
        #expect(effects.contains(.resignKey))
        #expect(machine.phase == .leaving)
    }

    @Test("typing on a show sends a message and leaves once peekd accepted it")
    func messageFlow() {
        var machine = F.visibleMachine(F.show("s"))
        let start = machine.handle(.keyboardButton, now: 1)
        #expect(start.contains(.makeKey))
        #expect(start.sent == [.focus])
        #expect(start.contains(.cancel(.autoDismiss)))
        _ = machine.handle(.typingChanged("  thanks!  "), now: 2)
        let submit = machine.handle(.submitTyping, now: 3)
        #expect(submit.sent == [.message("thanks!")])
        #expect(machine.phase == .typing, "stays until peekd replies")
        let replied = machine.handle(.replied(ok: true, message: nil), now: 3.1)
        #expect(replied.contains(.slideOut))
    }

    @Test("an empty message is never sent")
    func emptyTypingIgnored() {
        var machine = F.visibleMachine(F.show("s"))
        _ = machine.handle(.startTyping(seed: nil), now: 1)
        _ = machine.handle(.typingChanged("   "), now: 2)
        #expect(machine.handle(.submitTyping, now: 3).isEmpty)
    }

    @Test("a refused request keeps the bubble and shows why")
    func refusedAnswer() {
        var machine = F.visibleMachine(F.ask("s", kind: F.keepDelete))
        _ = machine.handle(.chooseOption("keep", via: .click), now: 1)
        #expect(machine.ask == .submitting)
        let effects = machine.handle(.replied(ok: false, message: "Not sent: peekd isn't reachable."), now: 1.1)
        #expect(machine.ask == .open)
        #expect(machine.notice == "Not sent: peekd isn't reachable.")
        #expect(effects.schedules(.notice) == 4)
        #expect(!effects.contains(.slideOut))
    }

    @Test("toggling the keyboard off returns to the bubble and re-arms the auto-dismiss")
    func keyboardToggle() {
        var machine = F.visibleMachine(F.show("s", durationMs: 5000))
        _ = machine.handle(.keyboardButton, now: 1)
        let off = machine.handle(.keyboardButton, now: 2)
        #expect(machine.phase == .showing)
        #expect(off.contains(.resignKey))
        #expect(off.schedules(.autoDismiss) == 5)
    }

    // MARK: Asks

    @Test("single choice: a click answers, the drawing gets `answer`, and the bubble leaves")
    func singleChoice() {
        var machine = F.visibleMachine(F.ask("s", kind: F.keepDelete))
        #expect(machine.phase == .asking)
        #expect(machine.askValue == nil, "nothing is selected before the Carbon picks")
        let click = machine.handle(.chooseOption("delete", via: .click), now: 1)
        #expect(click.sent == [.answer(.choice("delete"), via: .click)])
        let replied = machine.handle(.replied(ok: true, message: nil), now: 1.05)
        #expect(replied.contains(.drawing(.answer(value: .choice("delete"), via: .click))))
        #expect(replied.contains(.slideOut))
        #expect(machine.ask == .answered)
    }

    @Test("asks never auto-dismiss, even after speech")
    func asksStay() {
        var machine = F.visibleMachine(F.ask("s", kind: F.keepDelete, speak: "Should I delete it?"))
        _ = machine.handle(.speechStarted, now: 1)
        let effects = machine.handle(.speechFinished(stoppedByUser: false), now: 2)
        #expect(effects.schedules(.autoDismiss) == nil)
        #expect(machine.phase == .asking)
    }

    @Test("multiple choice honours min and max and submits in option order")
    func multipleChoice() {
        let options = ["a", "b", "c"].map { AskOption(id: $0, label: $0.uppercased()) }
        var machine = F.visibleMachine(F.ask("s", kind: .multipleChoice(options: options, min: 1, max: 2)))
        #expect(machine.askValue == .choices([]))
        let tooFew = machine.handle(.submitValue(via: .click), now: 1)
        #expect(tooFew.sent.isEmpty)
        #expect(machine.notice == "Pick at least one option")
        _ = machine.handle(.toggleOption("c"), now: 2)
        _ = machine.handle(.toggleOption("a"), now: 2)
        _ = machine.handle(.toggleOption("b"), now: 2)
        #expect(machine.askValue == .choices(["a", "c"]))
        #expect(machine.notice == "Pick at most 2")
        _ = machine.handle(.toggleOption("c"), now: 3)
        #expect(machine.askValue == .choices(["a"]))
        let submit = machine.handle(.submitValue(via: .keyboard), now: 4)
        #expect(submit.sent == [.answer(.choices(["a"]), via: .keyboard)])
    }

    @Test("slider starts at its default, follows drags, and submits the value")
    func slider() {
        var machine = F.visibleMachine(F.ask("s", kind: .slider(SliderSpec(min: 0, max: 100, step: 5, defaultValue: 50))))
        #expect(machine.askValue == .number(50))
        _ = machine.handle(.setValue(.number(75)), now: 1)
        let submit = machine.handle(.submitValue(via: .click), now: 2)
        #expect(submit.sent == [.answer(.number(75), via: .click)])
    }

    @Test("a typed answer to a choice ask is matched locally")
    func typedChoice() {
        var machine = F.visibleMachine(F.ask("s", kind: F.keepDelete))
        _ = machine.handle(.startTyping(seed: "2"), now: 1)
        let submit = machine.handle(.submitTyping, now: 2)
        #expect(submit.sent == [.answer(.choice("delete"), via: .keyboard)])

        var other = F.visibleMachine(F.ask("s", kind: F.keepDelete))
        _ = other.handle(.startTyping(seed: "maybe"), now: 1)
        let unmatched = other.handle(.submitTyping, now: 2)
        #expect(unmatched.sent.isEmpty)
        #expect(other.notice == BubbleMachine.didNotMatchNotice)
        #expect(other.phase == .typing)
    }

    @Test("a text ask answered by typing sends the text as the answer")
    func textAskTyped() {
        var machine = F.visibleMachine(F.ask("s", kind: .text(placeholder: nil, maxLength: 5)))
        _ = machine.handle(.startTyping(seed: nil), now: 1)
        _ = machine.handle(.typingChanged("abcdefgh"), now: 1)
        #expect(machine.typingText == "abcde", "capped at max_length")
        #expect(machine.handle(.submitTyping, now: 2).sent == [.answer(.text("abcde"), via: .keyboard)])
    }

    // MARK: Voice

    @Test("voice on a text ask: record, stop, upload, then leave (the transcript is delivered in the background)")
    func voiceTextAsk() {
        var machine = F.visibleMachine(F.ask("s", kind: .text(placeholder: nil, maxLength: 500)))
        let start = machine.handle(.micButton, now: 1)
        #expect(start.contains(.startRecording))
        #expect(start.contains(.makeKey))
        #expect(machine.phase == .listening)
        let stop = machine.handle(.micButton, now: 3)
        #expect(stop == [.stopRecording])
        let recorded = F.recording()
        let upload = machine.handle(.recordingFinished(recorded), now: 3.1)
        #expect(upload.sent == [.voice(recorded)])
        #expect(upload.schedules(.transcribe) == nil)
        #expect(machine.phase == .transcribing)
        let replied = machine.handle(.replied(ok: true, message: nil), now: 3.2)
        #expect(replied.contains(.slideOut))
    }

    @Test("silence (below −50 dBFS) is never uploaded; the bubble says nothing was heard")
    func silentRecording() {
        var machine = F.visibleMachine(F.ask("s", kind: .text(placeholder: nil, maxLength: 500)))
        _ = machine.handle(.micButton, now: 1)
        _ = machine.handle(.micButton, now: 2)
        let effects = machine.handle(.recordingFinished(F.recording(silent: true)), now: 2.1)
        #expect(effects.sent.isEmpty)
        #expect(machine.notice == BubbleMachine.nothingHeardNotice)
        #expect(machine.phase == .asking)
    }

    @Test("voice on a choice ask waits in `transcribing` for stt.result: matched leaves, unmatched and failed explain")
    func voiceChoice() {
        var machine = F.visibleMachine(F.ask("s", kind: F.keepDelete))
        _ = machine.handle(.micButton, now: 1)
        _ = machine.handle(.micButton, now: 2)
        let upload = machine.handle(.recordingFinished(F.recording()), now: 2.1)
        #expect(upload.schedules(.transcribe) == 8)
        #expect(machine.handle(.replied(ok: true, message: nil), now: 2.2).isEmpty)
        #expect(machine.phase == .transcribing)
        let unmatched = machine.handle(.sttResult(.unmatched, value: nil), now: 3)
        #expect(unmatched.contains(.cancel(.transcribe)))
        #expect(machine.notice == BubbleMachine.didNotMatchNotice)
        #expect(machine.phase == .asking)

        _ = machine.handle(.micButton, now: 4)
        _ = machine.handle(.micButton, now: 5)
        _ = machine.handle(.recordingFinished(F.recording()), now: 5.1)
        _ = machine.handle(.replied(ok: true, message: nil), now: 5.2)
        let failed = machine.handle(.sttResult(.failed, value: nil), now: 6)
        #expect(failed.contains(.cancel(.transcribe)))
        #expect(machine.notice == BubbleMachine.transcribeFailedNotice)

        _ = machine.handle(.micButton, now: 7)
        _ = machine.handle(.micButton, now: 8)
        _ = machine.handle(.recordingFinished(F.recording()), now: 8.1)
        let matched = machine.handle(.sttResult(.matched, value: .string("keep")), now: 9)
        #expect(matched.contains(.drawing(.answer(value: .choice("keep"), via: .voice))))
        #expect(matched.contains(.slideOut))
        #expect(machine.ask == .answered)
    }

    @Test("`transcribing` lasts at most 8 s, then the ask is back with a notice")
    func transcribeTimeout() {
        var machine = F.visibleMachine(F.ask("s", kind: F.keepDelete))
        _ = machine.handle(.micButton, now: 1)
        _ = machine.handle(.micButton, now: 2)
        _ = machine.handle(.recordingFinished(F.recording()), now: 2.1)
        _ = machine.handle(.timerFired(.transcribe), now: 10.1)
        #expect(machine.phase == .asking)
        #expect(machine.notice == BubbleMachine.stillTranscribingNotice)
        // A late match still answers and leaves.
        #expect(machine.handle(.sttResult(.matched, value: .string("delete")), now: 11).contains(.slideOut))
    }

    @Test("a denied microphone explains how to fix it")
    func micDenied() {
        var machine = F.visibleMachine(F.show("s", durationMs: 3000))
        _ = machine.handle(.micButton, now: 1)
        let effects = machine.handle(.recordingFailed("Peek can't use the microphone."), now: 1.1)
        #expect(machine.notice == "Peek can't use the microphone.")
        #expect(machine.phase == .showing)
        #expect(effects.schedules(.notice) == 4)
    }

    @Test("a voice message on a show uploads with no ask and leaves")
    func voiceMessage() {
        var machine = F.visibleMachine(F.show("s"))
        _ = machine.handle(.micButton, now: 1)
        _ = machine.handle(.micButton, now: 2)
        let upload = machine.handle(.recordingFinished(F.recording()), now: 2.1)
        #expect(upload.sent.count == 1)
        #expect(machine.handle(.replied(ok: true, message: nil), now: 2.2).contains(.slideOut))
    }

    // MARK: peekd and scheduler driven exits

    @Test("peek.cancel slides out silently and stops the audio")
    func cancelled() {
        var machine = F.visibleMachine(F.show("s", speak: "hi"))
        let effects = machine.handle(.cancelled, now: 1)
        #expect(effects.contains(.slideOut))
        #expect(effects.contains(.stopSpeech))
        #expect(effects.sent.isEmpty)
    }

    @Test("cancel before the slide-in finishes the bubble at once")
    func cancelledWhilePending() {
        var machine = BubbleMachine(id: .send("s"), source: .send(F.show("s")))
        #expect(machine.handle(.cancelled, now: 0).finishedRequeue == false)
    }

    @Test("pre-emption slides out, keeps the ask pending and asks to be queued again")
    func preempted() {
        var machine = F.visibleMachine(F.ask("s", context: .testing(environmentID: "e"), kind: F.keepDelete))
        let effects = machine.handle(.preempted, now: 1)
        #expect(effects.contains(.slideOut))
        #expect(effects.sent.isEmpty)
        #expect(machine.ask == .open)
        #expect(machine.handle(.timerFired(.leave), now: 1.5).finishedRequeue == true)
    }

    @Test("a newer show replaces this one: audio stops and shown.done(auto) is reported")
    func replaced() {
        var machine = F.visibleMachine(F.show("s", speak: "hi"))
        let effects = machine.handle(.replaced, now: 2)
        #expect(effects.contains(.stopSpeech))
        #expect(effects.sent == [.shownDone(.auto, visibleMs: 2000)])
    }

    @Test("a test bubble shows 'Sent to test silicon' before it leaves")
    func testContextLinger() {
        var machine = F.visibleMachine(F.ask("s", context: .testing(environmentID: "e"), kind: F.keepDelete))
        _ = machine.handle(.chooseOption("keep", via: .click), now: 1)
        let replied = machine.handle(.replied(ok: true, message: nil), now: 1.1)
        #expect(machine.notice == BubbleMachine.sentToTestNotice)
        #expect(replied.schedules(.sentLinger) == 0.9)
        #expect(!replied.contains(.slideOut))
        #expect(machine.handle(.timerFired(.sentLinger), now: 2).contains(.slideOut))
    }

    // MARK: Summons and key focus

    @Test("a summon takes key focus, pre-warms peekd, and slides back after 12 s of nothing")
    func summonIdle() {
        var machine = BubbleMachine(id: .summon(.left, serial: 1), source: .summon(.left))
        let begin = machine.handle(.begin, now: 0)
        #expect(begin.contains(.makeKey))
        #expect(begin.sent == [.focus])
        #expect(begin.schedules(.summonIdle) == 12)
        #expect(machine.wantsKey)
        _ = machine.handle(.timerFired(.enter), now: 0.5)
        #expect(machine.phase == .showing)
        #expect(machine.handle(.timerFired(.summonIdle), now: 12).contains(.slideOut))
    }

    @Test("the hotkey on a visible show makes it key and pauses its auto-dismiss")
    func summonExisting() {
        var machine = F.visibleMachine(F.show("s"))
        #expect(!machine.wantsKey, "Silicon-initiated peeks never take key focus")
        let effects = machine.handle(.summoned, now: 1)
        #expect(effects.contains(.makeKey))
        #expect(effects.contains(.cancel(.autoDismiss)))
        #expect(machine.wantsKey)
        let idle = machine.handle(.timerFired(.summonIdle), now: 13)
        #expect(idle.contains(.resignKey))
        #expect(idle.schedules(.autoDismiss) != nil)
    }

    @Test("losing key focus with an empty field returns to the bubble; typed text is kept")
    func resignKey() {
        var machine = F.visibleMachine(F.show("s"))
        _ = machine.handle(.startTyping(seed: nil), now: 1)
        _ = machine.handle(.resignedKey, now: 2)
        #expect(machine.phase == .showing)

        var typing = F.visibleMachine(F.show("s"))
        _ = typing.handle(.startTyping(seed: "draft"), now: 1)
        _ = typing.handle(.resignedKey, now: 2)
        #expect(typing.phase == .typing)
        #expect(typing.typingText == "draft")
    }

    @Test("phases cover every InputSnapshot phase the machine can produce")
    func phaseCoverage() {
        var seen: Set<Phase> = []
        var machine = BubbleMachine(id: .send("s"), source: .send(F.ask("s", kind: F.keepDelete, speak: "hi")))
        seen.insert(machine.phase)
        _ = machine.handle(.begin, now: 0)
        seen.insert(machine.phase)
        _ = machine.handle(.timerFired(.enter), now: 0.5)
        seen.insert(machine.phase)
        _ = machine.handle(.speechStarted, now: 1)
        seen.insert(machine.phase)
        _ = machine.handle(.speechFinished(stoppedByUser: false), now: 2)
        seen.insert(machine.phase)
        _ = machine.handle(.startTyping(seed: nil), now: 3)
        seen.insert(machine.phase)
        _ = machine.handle(.micButton, now: 4)
        seen.insert(machine.phase)
        _ = machine.handle(.micButton, now: 5)
        _ = machine.handle(.recordingFinished(F.recording()), now: 5.1)
        seen.insert(machine.phase)
        _ = machine.handle(.escape, now: 6)
        seen.insert(machine.phase)
        var show = F.visibleMachine(F.show("t"))
        seen.insert(show.phase)
        _ = show.handle(.cancelled, now: 1)
        #expect(seen == Set(Phase.allCases))
    }
}
