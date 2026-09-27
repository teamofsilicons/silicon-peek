import Foundation
import PeekCore
import Testing

@testable import PeekUI

/// peek 0.1.2 bubble machine rows (contract §8.3): the pre-warm stage, Esc on shows/speaks and asks, the compact ask.
@Suite("Slot state machine: pre-warm, Esc and the compact ask")
struct SlotEscAndPrewarmTests {
    typealias F = SlotFixtures

    // MARK: Pre-warm

    @Test("begin pre-warms (enter to the drawing, shown to peekd, the 0.6 s cap); prewarmed slides in and opens Esc's grace")
    func prewarmOrdering() {
        var machine = BubbleMachine(id: .send("s"), source: .send(F.show("s", speak: "hi")))
        let begin = machine.handle(.begin, now: 5)
        #expect(begin == [.prewarm, .drawing(.enter), .send(.shown), .schedule(.prewarm, seconds: 0.6)])
        #expect(machine.stage == .prewarming)
        #expect(machine.visibleSince == nil)
        #expect(machine.graceUntil == nil)
        #expect(!machine.isOnScreen)

        let slide = machine.handle(.prewarmed, now: 5.4)
        #expect(Array(slide.prefix(3)) == [.cancel(.prewarm), .slideIn, .schedule(.enter, seconds: 0.5)])
        #expect(slide.schedules(.speechStart) == 8, "the speech-start budget counts from the slide")
        #expect(machine.stage == .entering)
        #expect(machine.visibleSince == 5.4)
        #expect(machine.graceUntil == 8.4)
        #expect(machine.handle(.prewarmed, now: 5.5).isEmpty, "only once")
    }

    @Test("the pre-warm cap slides the bubble in whatever is still missing")
    func prewarmTimerFallback() {
        var machine = BubbleMachine(id: .send("s"), source: .send(F.show("s")))
        _ = machine.handle(.begin, now: 0)
        let slide = machine.handle(.timerFired(.prewarm), now: 0.6)
        #expect(slide.contains(.slideIn))
        #expect(machine.stage == .entering)
        #expect(machine.handle(.timerFired(.prewarm), now: 0.7).isEmpty)
    }

    @Test("cancelled or pre-empted while pre-warming: never seen, no slide-out, audio stopped, requeued when pre-empted")
    func prewarmInterrupted() {
        var cancelled = BubbleMachine(id: .send("s"), source: .send(F.show("s", speak: "hi")))
        _ = cancelled.handle(.begin, now: 0)
        let effects = cancelled.handle(.cancelled, now: 0.1)
        #expect(effects.contains(.cancel(.prewarm)))
        #expect(effects.contains(.stopSpeech), "peekd already started the speech on `shown`")
        #expect(!effects.contains(.slideOut))
        #expect(effects.finishedRequeue == false)
        #expect(cancelled.stage == .finished)

        var preempted = BubbleMachine(id: .send("t"), source: .send(F.ask("t", context: .testing(environmentID: "e"), kind: F.keepDelete)))
        _ = preempted.handle(.begin, now: 0)
        let out = preempted.handle(.preempted, now: 0.1)
        #expect(out.finishedRequeue == true)
        #expect(!out.contains(.slideOut))
    }

    @Test("the hotkey or a typing offer during the pre-warm takes effect as the bubble slides in")
    func deferredUntilSlideIn() {
        var machine = BubbleMachine(id: .send("s"), source: .send(F.show("s")))
        _ = machine.handle(.begin, now: 0)
        #expect(machine.handle(.summoned, now: 0.1).isEmpty)
        let slide = machine.handle(.prewarmed, now: 0.2)
        #expect(slide.contains(.makeKey))
        #expect(slide.sent == [.focus])
        #expect(machine.wantsKey)

        var summon = BubbleMachine(id: .summon(.top, serial: 1), source: .summon(.top), speechAvailable: false)
        _ = summon.handle(.begin, now: 0)
        #expect(summon.handle(.offerTyping(notice: BubbleMachine.transcribeFailedNotice), now: 0.1).isEmpty)
        #expect(summon.input == .none)
        _ = summon.handle(.prewarmed, now: 0.2)
        #expect(summon.input == .typing)
        #expect(summon.notice == BubbleMachine.transcribeFailedNotice)
    }

    // MARK: Esc on shows and speaks

    @Test("show/speak: one Esc slides the visual out, keeps the audio and reports esc once the 0.4 s window closes")
    func singleEscKeepsAudio() {
        var machine = F.visibleMachine(F.show("s", speak: "a long intro"))
        _ = machine.handle(.speechStarted, now: 1)
        let esc = machine.handle(.escape, now: 2)
        #expect(esc.contains(.slideOut))
        #expect(!esc.contains(.stopSpeech))
        #expect(esc.sent.isEmpty)
        #expect(esc.schedules(.escDouble) == 0.4)
        #expect(machine.escArmedUntil == 2.4)
        #expect(machine.leaveReason == .escaped)
        #expect(machine.audioOutlivesBubble)
        #expect(machine.handle(.timerFired(.leave), now: 2.3).finishedRequeue == nil, "waits for the gesture")
        let window = machine.handle(.timerFired(.escDouble), now: 2.4)
        #expect(window.sent == [.dismissed(.esc)])
        #expect(window.finishedRequeue == false)
        #expect(machine.escArmedUntil == nil)
        #expect(machine.speech == .playing, "the audio plays on")
    }

    @Test("show/speak: two Escs within 0.4 s also stop the audio and report esc_double once")
    func doubleEscStopsAudio() {
        var machine = F.visibleMachine(F.show("s", speak: "a long intro"))
        _ = machine.handle(.speechStarted, now: 1)
        _ = machine.handle(.escape, now: 2)
        let second = machine.handle(.escape, now: 2.2)
        #expect(second == [.cancel(.escDouble), .stopSpeech, .send(.dismissed(.escDouble))])
        #expect(machine.speech == .stopped)
        #expect(machine.escArmedUntil == nil)
        #expect(machine.handle(.timerFired(.escDouble), now: 2.4).sent.isEmpty, "reported once")
        #expect(machine.handle(.escape, now: 2.5).isEmpty, "a third Esc changes nothing")
        #expect(machine.handle(.timerFired(.leave), now: 2.45).finishedRequeue == false)
    }

    @Test("show: an Esc after the down-arrow, or after the window closed, does nothing")
    func escOutsideItsWindow() {
        var machine = F.visibleMachine(F.show("s", speak: "hi"))
        _ = machine.handle(.downArrowClick, now: 1)
        #expect(machine.handle(.escape, now: 1.1).isEmpty)

        var late = F.visibleMachine(F.show("t", speak: "hi"))
        _ = late.handle(.escape, now: 1)
        _ = late.handle(.timerFired(.escDouble), now: 1.4)
        #expect(late.handle(.escape, now: 1.42).isEmpty)
    }

    @Test("show: a message already on its way is not dismissed by Esc; a summon's Esc sends nothing")
    func escWithoutDismissal() {
        var machine = F.visibleMachine(F.show("s"))
        _ = machine.handle(.startTyping(seed: "hi"), now: 1)
        _ = machine.handle(.submitTyping, now: 1.5)
        _ = machine.handle(.escape, now: 2)
        #expect(machine.handle(.timerFired(.escDouble), now: 2.4).sent.isEmpty)

        var summon = BubbleMachine(id: .summon(.top, serial: 1), source: .summon(.top), speechAvailable: false)
        _ = summon.beginAndSlide(now: 0)
        let esc = summon.handle(.escape, now: 1)
        #expect(esc.contains(.slideOut))
        #expect(esc.sent.isEmpty)
        #expect(esc.schedules(.escDouble) == nil)
    }

    // MARK: Esc on asks: the compact ask

    @Test("ask: one Esc collapses it (still answerable, phase asking); a second within 0.4 s dismisses it and stops the speech")
    func askEscCollapsesThenDismisses() {
        var machine = F.visibleMachine(F.ask("s", kind: F.keepDelete, speak: "Keep it?"))
        _ = machine.handle(.speechStarted, now: 1)
        let first = machine.handle(.escape, now: 2)
        #expect(first == [.schedule(.escDouble, seconds: 0.4)])
        #expect(machine.askPresentation == .collapsed)
        #expect(machine.isAskCollapsed)
        #expect(machine.escArmedUntil == 2.4)
        #expect(machine.phase == .speaking || machine.phase == .asking)
        #expect(machine.isAskOpen)
        let second = machine.handle(.escape, now: 2.3)
        #expect(Array(second.prefix(4)) == [.cancel(.escDouble), .cancel(.escHint), .stopSpeech, .send(.dismissed(.esc))])
        #expect(second.contains(.slideOut))
        #expect(machine.speech == .stopped)
        #expect(machine.leaveReason == .escaped)
        #expect(machine.handle(.timerFired(.leave), now: 2.8).finishedRequeue == false)
    }

    @Test("ask: after the 0.4 s window the compact ask stays; an Esc shows the hint, a second within 2 s dismisses")
    func compactAskHintThenDismiss() {
        var machine = F.visibleMachine(F.ask("s", kind: F.keepDelete))
        _ = machine.handle(.escape, now: 1)
        #expect(machine.handle(.timerFired(.escDouble), now: 1.4).isEmpty)
        #expect(machine.escArmedUntil == nil)
        #expect(machine.isAskCollapsed, "stays compact")
        let hint = machine.handle(.escape, now: 3)
        #expect(hint == [.schedule(.escHint, seconds: 2)])
        #expect(machine.escHintVisible)
        #expect(machine.escArmedUntil == 5)
        let dismiss = machine.handle(.escape, now: 4)
        #expect(dismiss.sent == [.dismissed(.esc)])
        #expect(dismiss.contains(.slideOut))
        #expect(!machine.escHintVisible)
    }

    @Test("ask: the hint fades after 2 s and the ask stays compact and answerable")
    func compactAskHintFades() {
        var machine = F.visibleMachine(F.ask("s", kind: F.keepDelete))
        _ = machine.handle(.escape, now: 1)
        _ = machine.handle(.timerFired(.escDouble), now: 1.4)
        _ = machine.handle(.escape, now: 2)
        #expect(machine.handle(.timerFired(.escHint), now: 4).isEmpty)
        #expect(!machine.escHintVisible)
        #expect(machine.escArmedUntil == nil)
        #expect(machine.isAskCollapsed)
        #expect(machine.handle(.escape, now: 5) == [.schedule(.escHint, seconds: 2)], "the next Esc shows the hint again")
        let answer = machine.handle(.chooseOption("keep", via: .keyboard), now: 5.5)
        #expect(answer.sent == [.answer(.choice("keep"), via: .keyboard)], "a compact ask is still answerable")
    }

    @Test("ask: Esc while typing discards the text and collapses; nothing is uploaded")
    func escWhileTypingOnAsk() {
        var machine = F.visibleMachine(F.ask("s", kind: .text(placeholder: nil, maxLength: 60)))
        _ = machine.handle(.startTyping(seed: "Late"), now: 1)
        _ = machine.handle(.typingChanged("Late-night"), now: 1.2)
        let esc = machine.handle(.escape, now: 2)
        #expect(esc.contains(.resignKey))
        #expect(esc.sent.isEmpty)
        #expect(machine.typingText.isEmpty)
        #expect(machine.input == .none)
        #expect(machine.isAskCollapsed)
        #expect(!machine.wantsKey)
    }

    @Test("ask: Esc while recording cancels the mic; the recording is never uploaded")
    func escWhileRecordingOnAsk() {
        var machine = F.visibleMachine(F.ask("s", kind: F.keepDelete))
        _ = machine.handle(.micButton, now: 1)
        let esc = machine.handle(.escape, now: 2)
        #expect(esc.contains(.cancelRecording))
        #expect(machine.input == .none)
        #expect(machine.isAskCollapsed)
        #expect(machine.handle(.recordingFinished(F.recording()), now: 2.1).isEmpty, "a late WAV is dropped")
    }

    @Test("ask: with an answer in flight Esc only collapses (no window), and a compact ask in flight ignores Esc")
    func escWithAnswerInFlight() {
        var machine = F.visibleMachine(F.ask("s", kind: F.keepDelete))
        _ = machine.handle(.chooseOption("keep", via: .click), now: 1)
        #expect(machine.ask == .submitting)
        #expect(machine.handle(.escape, now: 1.1).isEmpty)
        #expect(machine.isAskCollapsed)
        #expect(machine.escArmedUntil == nil)
        #expect(machine.handle(.escape, now: 1.2).isEmpty)
        #expect(machine.stage == .visible)

        var voice = F.visibleMachine(F.ask("v", kind: F.keepDelete))
        _ = voice.handle(.micButton, now: 1)
        _ = voice.handle(.micButton, now: 2)
        _ = voice.handle(.recordingFinished(F.recording()), now: 2.1)
        #expect(voice.input == .transcribing)
        #expect(voice.handle(.escape, now: 2.2).isEmpty)
        #expect(voice.input == .transcribing, "the transcription is not thrown away")
    }

    @Test("down-arrow on a compact (or full) ask always dismisses; it never collapses")
    func downArrowDismissesAsks() {
        var compact = F.visibleMachine(F.ask("s", kind: F.keepDelete))
        _ = compact.handle(.escape, now: 1)
        _ = compact.handle(.timerFired(.escDouble), now: 1.4)
        let click = compact.handle(.downArrowClick, now: 2)
        #expect(click.contains(.slideOut))
        #expect(compact.handle(.timerFired(.doubleClick), now: 2.5).sent == [.dismissed(.downArrow)])

        var full = F.visibleMachine(F.ask("t", kind: F.keepDelete, speak: "Keep?"))
        _ = full.handle(.speechStarted, now: 1)
        _ = full.handle(.downArrowClick, now: 2)
        let double = full.handle(.downArrowDoubleClick, now: 2.2)
        #expect(double.contains(.stopSpeech))
        #expect(double.sent == [.dismissed(.downArrowDouble)])
        #expect(full.askPresentation == .expanded)
    }

    @Test("^ (expandAsk) brings the controls back and closes the Esc windows; the hotkey, keyboard and mic expand too")
    func expandAsk() {
        var machine = F.visibleMachine(F.ask("s", kind: F.keepDelete))
        _ = machine.handle(.escape, now: 1)
        _ = machine.handle(.timerFired(.escDouble), now: 1.4)
        _ = machine.handle(.escape, now: 2)
        let expand = machine.handle(.expandAsk, now: 2.5)
        #expect(expand == [.cancel(.escHint), .cancel(.escDouble)])
        #expect(machine.askPresentation == .expanded)
        #expect(!machine.escHintVisible)
        #expect(machine.escArmedUntil == nil)
        #expect(machine.handle(.expandAsk, now: 3).isEmpty)

        var summoned = F.visibleMachine(F.ask("t", kind: F.keepDelete))
        _ = summoned.handle(.escape, now: 1)
        let summon = summoned.handle(.summoned, now: 2)
        #expect(summoned.askPresentation == .expanded)
        #expect(summon.contains(.makeKey))

        var keyboard = F.visibleMachine(F.ask("u", kind: .text(placeholder: nil, maxLength: 60)))
        _ = keyboard.handle(.escape, now: 1)
        _ = keyboard.handle(.keyboardButton, now: 2)
        #expect(keyboard.askPresentation == .expanded)
        #expect(keyboard.input == .typing)

        var mic = F.visibleMachine(F.ask("v", kind: F.keepDelete))
        _ = mic.handle(.escape, now: 1)
        let record = mic.handle(.micButton, now: 2)
        #expect(mic.askPresentation == .expanded)
        #expect(record.contains(.startRecording))
    }

    @Test("a collapsed ask never auto-dismisses and a peek.cancel still takes it away silently")
    func collapsedAskStays() {
        var machine = F.visibleMachine(F.ask("s", kind: F.keepDelete))
        _ = machine.handle(.escape, now: 1)
        _ = machine.handle(.pointer(inside: true), now: 2)
        #expect(machine.handle(.pointer(inside: false), now: 3).schedules(.autoDismiss) == nil)
        let cancel = machine.handle(.cancelled, now: 4)
        #expect(cancel.contains(.slideOut))
        #expect(cancel.sent.isEmpty)
    }
}
