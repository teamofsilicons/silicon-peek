import Foundation
import PeekCore

// One bubble's life, as a pure state machine (no AppKit, no clocks, no I/O): events in, effects
// out. `SlotManager` runs the effects (slides the panel, starts timers, sends to peekd, records).
//
// Behaviour (understanding.md "Interactions", BLUEPRINT §1.9.3–§1.9.7, §8.5):
//   * a bubble slides in ("entering"), then shows, speaks, asks, listens, types or transcribes,
//     and slides out ("leaving");
//   * speak + show: slides back 1.5 s after speech is done (or `--duration` after it);
//     show only: slides back after `PeekShowEvent.effectiveDuration`; asks stay until answered;
//   * down-arrow: one click slides the bubble out and lets the audio play on; a double click also
//     stops the audio. The protocol message (`dismissed` with `down_arrow` or `down_arrow_double`)
//     is sent once, after the double-click window, so the gesture is reported exactly;
//   * Esc cancels typing/voice and slides the bubble back (`dismissed` with `esc`);
//   * answers, messages and voice recordings leave only once peekd accepted them, so nothing the
//     Carbon said is dropped silently; silent recordings are never uploaded (−50 dBFS guard);
//   * a voice answer to a choice/slider/range ask shows "transcribing" for up to 8 s and waits
//     for `stt.result` (matched → leave; unmatched/empty/failed → a notice, the ask stays).

/// Identifies a bubble: its send id, or a Carbon-initiated summon.
public struct BubbleID: Hashable, Sendable, CustomStringConvertible {
    public let raw: String
    public init(_ raw: String) { self.raw = raw }
    public static func send(_ sendID: String) -> BubbleID { BubbleID(sendID) }
    public static func summon(_ slot: SlotIndex, serial: Int) -> BubbleID { BubbleID("summon:\(slot.rawValue):\(serial)") }
    public var isSummon: Bool { raw.hasPrefix("summon:") }
    public var description: String { raw }
}

/// What a bubble presents.
public enum BubbleSource: Sendable, Equatable {
    case send(PeekShowEvent)
    /// The Carbon pressed the slot's hotkey with nothing on screen: an empty bubble to talk or type into.
    case summon(SlotIndex)

    public var event: PeekShowEvent? {
        if case .send(let event) = self { return event }
        return nil
    }
}

/// The durations the machine asks the manager to wait for (seconds).
public struct BubbleTiming: Sendable, Equatable {
    /// The slide-in spring's perceptual duration (CASpringAnimation(perceptualDuration: 0.5, bounce: 0.22)).
    public var enter: Double = 0.5
    /// The slide-out (`.smooth(duration: 0.3)`) plus its settling tail.
    public var leave: Double = 0.45
    /// Linger after speech ends when the send has no `--duration` (§1.9.3 step 6).
    public var afterSpeech: Double = 1.5
    /// No audio this long after the bubble appeared: show the text instead (peekd's own budget is 5 s).
    public var speechStartTimeout: Double = 8
    /// `transcribing` lasts at most this long (§1.9.5).
    public var transcribeTimeout: Double = 8
    public var noticeDuration: Double = 4
    /// A summoned bubble with nothing going on slides back after this long.
    public var summonIdle: Double = 12
    /// The double-click window (`NSEvent.doubleClickInterval`).
    public var doubleClick: Double = 0.5
    /// A test bubble shows "Sent to test silicon" this long before it leaves (gap-testing §9.3).
    public var sentLinger: Double = 0.9
    /// After the pointer leaves the bubble (or its popup closes), the show stays at least this long (ui-feedback.md:
    /// reading a revealed or expanded text must not be cut off by the auto-dismiss).
    public var afterHover: Double = 2.5

    public init() {}
}

public enum BubbleTimer: Hashable, Sendable, CaseIterable {
    case enter
    case leave
    case autoDismiss
    case speechStart
    case transcribe
    case notice
    case summonIdle
    case doubleClick
    case sentLinger
}

/// Something the bubble sends to peekd (the manager adds send id, ask id, slot and context).
public enum BubbleOutbound: Sendable, Equatable {
    case answer(AskValue, via: AnswerVia)
    case dismissed(DismissGesture)
    case shownDone(ShownDoneReason, visibleMs: Int)
    case message(String)
    case voice(MicRecordingResult)
    /// Pre-warm the session and the Deepgram token (`focus`).
    case focus
}

public enum BubbleEffect: Sendable, Equatable {
    case slideIn
    case slideOut
    case schedule(BubbleTimer, seconds: Double)
    case cancel(BubbleTimer)
    case send(BubbleOutbound)
    case stopSpeech
    case makeKey
    case resignKey
    case startRecording
    case stopRecording
    case cancelRecording
    case drawing(DrawingEvent)
    /// The bubble is gone: remove it; with `requeue` its send goes back to the slot's queue.
    case finished(requeue: Bool)
}

public enum BubbleEvent: Sendable, Equatable {
    /// Images are ready: slide in.
    case begin
    case speechStarted
    case speechFinished(stoppedByUser: Bool)
    case speechFailed
    case timerFired(BubbleTimer)
    case downArrowClick
    case downArrowDoubleClick
    case escape
    /// The live selection / slider value changed.
    case setValue(AskValue?)
    case setHighlight(String?)
    /// Single choice: a click (or Return on the highlight) answers at once.
    case chooseOption(String, via: AnswerVia)
    /// Multiple choice: toggles an option.
    case toggleOption(String)
    /// The ✓ (or Return): submits the current multiple-choice, slider or range value.
    case submitValue(via: AnswerVia)
    case keyboardButton
    case micButton
    case startTyping(seed: String?)
    case typingChanged(String)
    case submitTyping
    case recordingStarted
    case recordingFailed(String)
    case recordingFinished(MicRecordingResult)
    /// peekd's reply to the outstanding answer, message or voice upload.
    case replied(ok: Bool, message: String?)
    case sttResult(STTOutcome, value: JSONValue?)
    /// `peek.cancel`, or the app hides everything.
    case cancelled
    /// A production peek takes the physical slot; this (test) bubble goes back to the queue.
    case preempted
    /// A newer show from the same Silicon replaces this one.
    case replaced
    /// The hotkey was pressed again while this bubble is up.
    case summoned
    case resignedKey
    /// The pointer entered or left the bubble's chrome (or a popup opened/closed): the auto-dismiss pauses meanwhile.
    case pointer(inside: Bool)
    /// A voice message from this slot could not be transcribed: open typing with this notice.
    case offerTyping(notice: String)
}

public struct BubbleMachine: Sendable, Equatable {
    public enum Stage: Sendable, Equatable {
        case pending
        case entering
        case visible
        case leaving
        case finished
    }

    public enum Speech: Sendable, Equatable {
        case none
        case waiting
        case playing
        case finished
        case failed
        case stopped
    }

    public enum AskState: Sendable, Equatable {
        case none
        case open
        case submitting
        case answered
    }

    public enum Input: Sendable, Equatable {
        case none
        case typing
        /// Recording (or waiting for the mic to start).
        case listening
        /// Stopped; waiting for the WAV.
        case stopping
        /// Uploaded; waiting for peekd (and for `stt.result` on choice/slider/range asks).
        case transcribing
    }

    public enum Awaiting: Sendable, Equatable {
        case answer(AskValue, via: AnswerVia)
        case message
        case voice
    }

    public enum LeaveReason: Sendable, Equatable {
        case auto(ShownDoneReason)
        case dismissed
        case escaped
        case cancelled
        case preempted
        case replaced
        case answered
        case idle
    }

    public static let didNotMatchNotice = "Didn't match an option — tap one or type"
    public static let transcribeFailedNotice = "Couldn't transcribe — type instead"
    public static let nothingHeardNotice = "Didn't hear anything — try again or type"
    public static let stillTranscribingNotice = "Still transcribing — tap an option or type"
    public static let sentToTestNotice = "Sent to test silicon"

    public let id: BubbleID
    public let source: BubbleSource
    public let timing: BubbleTiming
    public private(set) var stage: Stage = .pending
    public private(set) var speech: Speech
    public private(set) var ask: AskState
    public private(set) var input: Input = .none
    public private(set) var askValue: AskValue?
    public private(set) var highlight: String?
    public private(set) var typingText = ""
    public private(set) var notice: String?
    /// The speak text is shown as a pill (TTS unavailable and nothing else to show, §1.9.3 step 7).
    public private(set) var speakAsPill: Bool
    /// The Carbon asked for this bubble (hotkey or a button): it may take key focus.
    public private(set) var summoned = false
    public private(set) var visibleSince: Double?
    public private(set) var leaveReason: LeaveReason?
    public private(set) var awaiting: Awaiting?
    public private(set) var requeue = false
    private var pendingDismiss: DismissGesture?
    private var leaveAnimationDone = false
    private var focusSent = false
    /// The pointer is over the chrome or a popup is open: the auto-dismiss waits.
    public private(set) var pointerInside = false
    /// When the armed (or deferred) auto-dismiss is due.
    private var autoDismissDeadline: Double?
    private var clock: Double = 0

    /// - Parameter speechAvailable: false when this send's audio was already consumed (a pre-empted
    ///   bubble shown again) so no speech is expected.
    public init(id: BubbleID, source: BubbleSource, timing: BubbleTiming = BubbleTiming(), speechAvailable: Bool = true) {
        self.id = id
        self.source = source
        self.timing = timing
        let event = source.event
        if let speak = event?.speak, speechAvailable, speak.status.expectsAudio {
            speech = .waiting
        } else {
            speech = .none
        }
        let speakText = event?.speak?.text ?? ""
        speakAsPill = speech == .none && !speakText.isEmpty && event?.show == nil && event?.ask == nil
        ask = event?.ask == nil ? .none : .open
        if let ask = event?.ask { askValue = AskValue.initial(for: ask) }
        if case .summon = source { summoned = true }
    }

    // MARK: Derived state

    public var event: PeekShowEvent? { source.event }
    public var askPayload: AskPayload? { event?.ask }
    public var isSummon: Bool {
        if case .summon = source { return true }
        return false
    }

    /// `input.phase` (visual.md A4 + `transcribing`).
    public var phase: Phase {
        switch stage {
        case .pending, .finished: return .hidden
        case .entering: return .entering
        case .leaving: return .leaving
        case .visible:
            switch input {
            case .listening, .stopping: return .listening
            case .transcribing: return .transcribing
            case .typing: return .typing
            case .none: break
            }
            if speech == .playing { return .speaking }
            if ask == .open || ask == .submitting { return .asking }
            return .showing
        }
    }

    /// Whether the Carbon is in the middle of answering (typing, recording or waiting on a transcript).
    public var hasUserInput: Bool { input != .none || awaiting != nil }
    public var isAskOpen: Bool { ask == .open || ask == .submitting }
    public var isOnScreen: Bool { stage == .entering || stage == .visible || stage == .leaving }
    /// The send's audio may still play after the bubble left (single down-arrow click).
    public var audioOutlivesBubble: Bool { speech == .waiting || speech == .playing }
    /// Whether the bubble may take key focus (canBecomeKey).
    public var wantsKey: Bool {
        guard stage == .entering || stage == .visible else { return false }
        return summoned || input != .none
    }

    /// Voice answers to these asks are matched by peekd and reported in `stt.result`.
    var voiceNeedsMatching: Bool {
        switch askPayload?.kind {
        case .singleChoice?, .multipleChoice?, .slider?, .range?: true
        default: false
        }
    }

    var speakCharacterCount: Int { event?.speak?.text.scalarCount ?? 0 }

    // MARK: Transitions

    public mutating func handle(_ event: BubbleEvent, now: Double) -> [BubbleEffect] {
        clock = now
        switch event {
        case .begin:
            guard stage == .pending else { return [] }
            stage = .entering
            visibleSince = now
            var effects: [BubbleEffect] = [.slideIn, .drawing(.enter), .schedule(.enter, seconds: timing.enter)]
            if speech == .waiting { effects.append(.schedule(.speechStart, seconds: timing.speechStartTimeout)) }
            if isSummon {
                effects.append(.makeKey)
                effects.append(contentsOf: sendFocusOnce())
                effects.append(.schedule(.summonIdle, seconds: timing.summonIdle))
            }
            return effects

        case .timerFired(let timer):
            return timerFired(timer, now: now)

        case .speechStarted:
            guard speech == .waiting else { return [] }
            speech = .playing
            return [.cancel(.speechStart)]

        case .speechFinished(let stoppedByUser):
            guard speech == .waiting || speech == .playing else { return [] }
            speech = stoppedByUser ? .stopped : .finished
            return [.cancel(.speechStart)] + armAutoDismiss()

        case .speechFailed:
            guard speech == .waiting || speech == .playing else { return [] }
            return speechFailed()

        case .downArrowClick:
            guard stage == .entering || stage == .visible else { return [] }
            if isSummon { return beginLeaving(.dismissed, now: now) }
            pendingDismiss = .downArrow
            return beginLeaving(.dismissed, now: now) + [.schedule(.doubleClick, seconds: timing.doubleClick)]

        case .downArrowDoubleClick:
            if pendingDismiss == .downArrow {
                pendingDismiss = nil
                var effects: [BubbleEffect] = [.cancel(.doubleClick), .stopSpeech,
                                               .send(.dismissed(.downArrowDouble))]
                if speech == .waiting || speech == .playing { speech = .stopped }
                effects.append(contentsOf: finishIfDone())
                return effects
            }
            guard stage == .entering || stage == .visible, !isSummon else { return [] }
            if speech == .waiting || speech == .playing { speech = .stopped }
            return beginLeaving(.dismissed, now: now) + [.stopSpeech, .send(.dismissed(.downArrowDouble))]

        case .escape:
            guard stage == .entering || stage == .visible else { return [] }
            var effects: [BubbleEffect] = []
            if input == .listening || input == .stopping { effects.append(.cancelRecording) }
            let answered = input == .transcribing || awaiting != nil
            input = .none
            effects.append(contentsOf: beginLeaving(.escaped, now: now))
            if !isSummon, !answered { effects.append(.send(.dismissed(.esc))) }
            return effects

        case .setValue(let value):
            guard isAskOpen else { return [] }
            askValue = value
            return []

        case .setHighlight(let id):
            guard isAskOpen else { return [] }
            highlight = id
            return []

        case .chooseOption(let id, let via):
            guard ask == .open, case .singleChoice(let options)? = askPayload?.kind, options.contains(where: { $0.id == id })
            else { return [] }
            askValue = .choice(id)
            return submit(.answer(.choice(id), via: via))

        case .toggleOption(let id):
            guard ask == .open, case .multipleChoice(let options, _, let max)? = askPayload?.kind,
                options.contains(where: { $0.id == id })
            else { return [] }
            var chosen: [String]
            if case .choices(let ids)? = askValue { chosen = ids } else { chosen = [] }
            if let index = chosen.firstIndex(of: id) {
                chosen.remove(at: index)
            } else {
                guard chosen.count < max else { return showNotice(max == 1 ? "Pick one option" : "Pick at most \(max)") }
                // Keep the options' order so the answer reads naturally.
                chosen.append(id)
                chosen.sort { a, b in
                    (options.firstIndex { $0.id == a } ?? 0) < (options.firstIndex { $0.id == b } ?? 0)
                }
            }
            askValue = .choices(chosen)
            return []

        case .submitValue(let via):
            guard ask == .open, let payload = askPayload else { return [] }
            switch payload.kind {
            case .multipleChoice(_, let min, let max):
                let count: Int
                if case .choices(let ids)? = askValue { count = ids.count } else { count = 0 }
                if count < min { return showNotice(min == 1 ? "Pick at least one option" : "Pick at least \(min)") }
                if count > max { return showNotice("Pick at most \(max)") }
                return submit(.answer(askValue ?? .choices([]), via: via))
            case .singleChoice:
                let chosen: String?
                if case .choice(let id)? = askValue { chosen = id } else { chosen = highlight }
                guard let chosen else { return showNotice("Pick an option first") }
                askValue = .choice(chosen)
                return submit(.answer(.choice(chosen), via: via))
            case .slider, .range:
                guard let value = askValue else { return [] }
                return submit(.answer(value, via: via))
            case .text:
                return []  // text asks submit through typing (.submitTyping)
            }

        case .keyboardButton:
            guard stage == .entering || stage == .visible, awaiting == nil else { return [] }
            if input == .typing { return stopTyping() }
            return startTyping(seed: nil)

        case .startTyping(let seed):
            guard stage == .entering || stage == .visible, awaiting == nil, input != .transcribing else { return [] }
            if input == .typing {
                if let seed { typingText += seed }
                return []
            }
            return startTyping(seed: seed)

        case .typingChanged(let text):
            guard input == .typing else { return [] }
            typingText = String(text.prefix(maxTypingLength))
            return []

        case .submitTyping:
            guard input == .typing, awaiting == nil else { return [] }
            let text = typingText.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !text.isEmpty else { return [] }  // an empty answer is never sent
            if let payload = askPayload, ask == .open {
                if case .text = payload.kind { return submit(.answer(.text(text), via: .keyboard)) }
                guard let value = TypedAnswerMatcher.match(text, ask: payload) else {
                    return showNotice(Self.didNotMatchNotice)
                }
                askValue = value
                return submit(.answer(value, via: .keyboard))
            }
            return submit(.message(text))

        case .micButton:
            guard stage == .entering || stage == .visible else { return [] }
            switch input {
            case .listening:
                input = .stopping
                return [.stopRecording]
            case .stopping, .transcribing:
                return []
            case .typing, .none:
                guard awaiting == nil else { return [] }
                input = .listening
                summoned = true
                notice = nil
                return [.makeKey, .startRecording, .cancel(.autoDismiss), .cancel(.summonIdle), .cancel(.notice)]
                    + sendFocusOnce()
            }

        case .recordingStarted:
            return []

        case .recordingFailed(let message):
            guard input == .listening || input == .stopping else { return [] }
            input = .none
            return showNotice(message) + armAutoDismiss()

        case .recordingFinished(let result):
            guard input == .listening || input == .stopping else { return [] }
            if result.isSilent {
                input = .none
                return showNotice(Self.nothingHeardNotice)
            }
            input = .transcribing
            awaiting = .voice
            var effects: [BubbleEffect] = [.send(.voice(result))]
            if voiceNeedsMatching && ask == .open {
                effects.append(.schedule(.transcribe, seconds: timing.transcribeTimeout))
            }
            return effects

        case .replied(let ok, let message):
            guard let pending = awaiting else { return [] }
            awaiting = nil
            guard ok else {
                if ask == .submitting { ask = .open }
                if input == .transcribing { input = .none }
                return [.cancel(.transcribe)] + showNotice(message ?? "peekd did not accept that; try again")
            }
            switch pending {
            case .answer(let value, let via):
                ask = .answered
                input = .none
                return [.drawing(.answer(value: value, via: via))] + leaveAfterSending(now: now)
            case .message:
                input = .none
                return leaveAfterSending(now: now)
            case .voice:
                if voiceNeedsMatching && ask == .open { return [] }  // stay "transcribing" for stt.result
                input = .none
                if ask == .open { ask = .answered }
                return leaveAfterSending(now: now)
            }

        case .sttResult(let outcome, let value):
            guard stage == .entering || stage == .visible, ask == .open || ask == .submitting else { return [] }
            switch outcome {
            case .matched:
                ask = .answered
                input = .none
                awaiting = nil
                var effects: [BubbleEffect] = [.cancel(.transcribe)]
                if let value, let type = askPayload?.type, let parsed = AskValue(json: value, for: type) {
                    askValue = parsed
                    effects.append(.drawing(.answer(value: parsed, via: .voice)))
                }
                return effects + beginLeaving(.answered, now: now)
            case .unmatched, .empty:
                guard input == .transcribing || awaiting == .voice else { return [] }
                input = .none
                awaiting = nil
                return [.cancel(.transcribe)] + showNotice(Self.didNotMatchNotice)
            case .failed, .other:
                guard input == .transcribing || awaiting == .voice else { return [] }
                input = .none
                awaiting = nil
                return [.cancel(.transcribe)] + showNotice(Self.transcribeFailedNotice)
            }

        case .cancelled:
            switch stage {
            case .pending:
                stage = .finished
                return [.finished(requeue: false)]
            case .entering, .visible:
                var effects: [BubbleEffect] = []
                if input == .listening || input == .stopping { effects.append(.cancelRecording) }
                if speech == .waiting || speech == .playing {
                    speech = .stopped
                    effects.append(.stopSpeech)
                }
                input = .none
                return effects + beginLeaving(.cancelled, now: now)
            case .leaving:
                // Already leaving (e.g. dismissed with the audio playing on): stop that audio too.
                if speech == .waiting || speech == .playing {
                    speech = .stopped
                    return [.stopSpeech]
                }
                return []
            case .finished:
                return []
            }

        case .preempted:
            guard stage == .entering || stage == .visible else {
                if stage == .pending {
                    stage = .finished
                    requeue = true
                    return [.finished(requeue: true)]
                }
                return []
            }
            requeue = true
            var effects: [BubbleEffect] = []
            if input == .listening || input == .stopping { effects.append(.cancelRecording) }
            if speech == .waiting || speech == .playing {
                speech = .stopped
                effects.append(.stopSpeech)
            }
            input = .none
            return effects + beginLeaving(.preempted, now: now)

        case .replaced:
            guard stage == .entering || stage == .visible else {
                if stage == .pending {
                    stage = .finished
                    return [.finished(requeue: false)]
                }
                return []
            }
            var effects: [BubbleEffect] = []
            if speech == .waiting || speech == .playing {
                speech = .stopped
                effects.append(.stopSpeech)
            }
            if !isSummon, let since = visibleSince {
                effects.append(.send(.shownDone(.auto, visibleMs: Self.milliseconds(now - since))))
            }
            return effects + beginLeaving(.replaced, now: now)

        case .summoned:
            guard stage == .entering || stage == .visible else { return [] }
            summoned = true
            var effects: [BubbleEffect] = [.makeKey] + sendFocusOnce()
            if input == .none {
                effects.append(.cancel(.autoDismiss))
                effects.append(.schedule(.summonIdle, seconds: timing.summonIdle))
            }
            return effects

        case .offerTyping(let text):
            guard stage == .entering || stage == .visible, input == .none, awaiting == nil else { return [] }
            return startTyping(seed: nil) + showNotice(text)

        case .pointer(let inside):
            guard pointerInside != inside else { return [] }
            pointerInside = inside
            guard stage == .visible else { return [] }
            if inside { return autoDismissDeadline == nil ? [] : [.cancel(.autoDismiss)] }
            let remaining = autoDismissDeadline.map { $0 - now }
            return armAutoDismiss(atLeast: timing.afterHover, remaining: remaining)

        case .resignedKey:
            guard stage == .entering || stage == .visible else { return [] }
            if input == .typing, typingText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                input = .none
            }
            guard input == .none, awaiting == nil else { return [] }
            summoned = false
            if isSummon, stage == .visible || stage == .entering { return beginLeaving(.idle, now: now) }
            return armAutoDismiss()
        }
    }

    // MARK: Helpers

    private var maxTypingLength: Int {
        if case .text(_, let maxLength)? = askPayload?.kind, ask == .open { return maxLength }
        return 2000
    }

    private mutating func timerFired(_ timer: BubbleTimer, now: Double) -> [BubbleEffect] {
        switch timer {
        case .enter:
            guard stage == .entering else { return [] }
            stage = .visible
            return armAutoDismiss()
        case .autoDismiss:
            guard stage == .visible, !hasUserInput, !isAskOpen, !pointerInside else { return [] }
            autoDismissDeadline = nil
            let reason: ShownDoneReason = speech == .finished ? .speechDone : .auto
            var effects: [BubbleEffect] = []
            if !isSummon, let since = visibleSince {
                effects.append(.send(.shownDone(reason, visibleMs: Self.milliseconds(now - since))))
            }
            return effects + beginLeaving(.auto(reason), now: now)
        case .speechStart:
            guard speech == .waiting else { return [] }
            return [.stopSpeech] + speechFailed()
        case .transcribe:
            guard input == .transcribing else { return [] }
            input = .none
            if awaiting == .voice { awaiting = nil }
            return showNotice(Self.stillTranscribingNotice)
        case .notice:
            notice = nil
            return []
        case .summonIdle:
            guard stage == .entering || stage == .visible, input == .none, awaiting == nil else { return [] }
            if isSummon { return beginLeaving(.idle, now: now) }
            summoned = false
            return [.resignKey] + armAutoDismiss()
        case .doubleClick:
            guard pendingDismiss == .downArrow else { return [] }
            pendingDismiss = nil
            return [.send(.dismissed(.downArrow))] + finishIfDone()
        case .leave:
            guard stage == .leaving else { return [] }
            leaveAnimationDone = true
            return finishIfDone()
        case .sentLinger:
            guard stage == .entering || stage == .visible else { return [] }
            return beginLeaving(.answered, now: now)
        }
    }

    private mutating func speechFailed() -> [BubbleEffect] {
        speech = .failed
        if let text = event?.speak?.text, !text.isEmpty, event?.show == nil, event?.ask == nil { speakAsPill = true }
        return [.cancel(.speechStart)] + armAutoDismiss()
    }

    /// Schedules the automatic slide-back when nothing keeps the bubble up. While the pointer is inside, only the
    /// deadline is remembered; leaving re-arms it with at least `atLeast` seconds (`remaining`: what was left).
    private mutating func armAutoDismiss(atLeast minimum: Double? = nil, remaining: Double? = nil) -> [BubbleEffect] {
        guard stage == .visible, !isSummon, !isAskOpen, ask != .answered, !hasUserInput, !summoned else { return [] }
        guard speech != .waiting, speech != .playing else { return [] }
        if let minimum, let remaining {
            let seconds = max(minimum, remaining)
            autoDismissDeadline = clock + seconds
            return [.schedule(.autoDismiss, seconds: seconds)]
        }
        let seconds: Double
        if speech == .finished || speech == .stopped {
            seconds = event?.durationMs.map { Double($0) / 1000 } ?? timing.afterSpeech
        } else if speakAsPill, event?.show == nil {
            seconds = event?.durationMs.map { Double($0) / 1000 }
                ?? min(max(3 + 0.06 * Double(speakCharacterCount), 4), 15)
        } else if let event {
            seconds = Double(event.effectiveDuration.components.seconds)
                + Double(event.effectiveDuration.components.attoseconds) / 1e18
        } else {
            seconds = timing.afterSpeech
        }
        autoDismissDeadline = clock + max(0, seconds)
        if pointerInside { return [] }
        return [.schedule(.autoDismiss, seconds: max(max(0, seconds), minimum ?? 0))]
    }

    private mutating func startTyping(seed: String?) -> [BubbleEffect] {
        var effects: [BubbleEffect] = []
        if input == .listening || input == .stopping { effects.append(.cancelRecording) }
        input = .typing
        typingText = String((seed ?? "").prefix(maxTypingLength))
        summoned = true
        notice = nil
        effects.append(contentsOf: [.makeKey, .cancel(.autoDismiss), .cancel(.summonIdle), .cancel(.notice)])
        effects.append(contentsOf: sendFocusOnce())
        return effects
    }

    private mutating func stopTyping() -> [BubbleEffect] {
        input = .none
        typingText = ""
        var effects: [BubbleEffect] = []
        if isSummon {
            effects.append(.schedule(.summonIdle, seconds: timing.summonIdle))
        } else {
            summoned = false
            effects.append(.resignKey)
            effects.append(contentsOf: armAutoDismiss())
        }
        return effects
    }

    private mutating func submit(_ outbound: BubbleOutbound) -> [BubbleEffect] {
        switch outbound {
        case .answer(let value, let via):
            ask = .submitting
            awaiting = .answer(value, via: via)
        case .message:
            awaiting = .message
        default:
            break
        }
        notice = nil
        return [.send(outbound), .cancel(.notice)]
    }

    private mutating func leaveAfterSending(now: Double) -> [BubbleEffect] {
        if event?.context.inputContext == .testing {
            notice = Self.sentToTestNotice
            return [.schedule(.sentLinger, seconds: timing.sentLinger)]
        }
        return beginLeaving(.answered, now: now)
    }

    private mutating func showNotice(_ text: String) -> [BubbleEffect] {
        notice = text
        return [.schedule(.notice, seconds: timing.noticeDuration)]
    }

    private mutating func sendFocusOnce() -> [BubbleEffect] {
        guard !focusSent else { return [] }
        focusSent = true
        return [.send(.focus)]
    }

    private mutating func beginLeaving(_ reason: LeaveReason, now: Double) -> [BubbleEffect] {
        guard stage == .entering || stage == .visible else { return [] }
        stage = .leaving
        leaveReason = reason
        let wasKey = summoned || input != .none
        summoned = false
        var effects: [BubbleEffect] = [.slideOut, .drawing(.leave), .schedule(.leave, seconds: timing.leave)]
        // The mic never stays on behind a bubble that left (e.g. the down-arrow while recording).
        if input == .listening || input == .stopping {
            effects.append(.cancelRecording)
            input = .none
        }
        for timer in [BubbleTimer.enter, .autoDismiss, .speechStart, .transcribe, .summonIdle, .sentLinger] {
            effects.append(.cancel(timer))
        }
        if wasKey { effects.append(.resignKey) }
        return effects
    }

    private mutating func finishIfDone() -> [BubbleEffect] {
        guard stage == .leaving, leaveAnimationDone, pendingDismiss == nil else { return [] }
        stage = .finished
        return [.finished(requeue: requeue)]
    }

    static func milliseconds(_ seconds: Double) -> Int { max(0, Int((seconds * 1000).rounded())) }
}
