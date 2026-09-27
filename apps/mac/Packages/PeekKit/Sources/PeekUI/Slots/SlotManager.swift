import AppKit
import OSLog
import PeekCore

/// Where a bubble's outbound request goes: everything the coordinator needs to build the IPC request.
public struct BubbleContext: Sendable, Equatable {
    public var sendID: String?
    public var askID: String?
    public var slot: SlotIndex
    public var key: SiliconKey
    public var askType: AskType?

    public var context: PeekContext { key.context }
}

/// peekd's answer to a bubble's outbound request.
public struct BubbleDelivery: Sendable, Equatable {
    /// nil when peekd took the request; otherwise a sentence for the Carbon saying what went wrong.
    public var problem: String?
    /// For a voice message (`voice.submit` with no ask): the message id peekd's reply named, which its later
    /// `stt.result` carries. nil for everything else, and from an older peekd (messages are then matched in order).
    public var messageID: String?

    public init(problem: String? = nil, messageID: String? = nil) {
        self.problem = problem
        self.messageID = messageID
    }

    /// peekd took the request.
    public static let delivered = BubbleDelivery()

    /// peekd did not take the request.
    public static func failed(_ problem: String) -> BubbleDelivery { BubbleDelivery(problem: problem) }
}

/// What the slot manager needs from the rest of the app (all injectable for tests).
@MainActor
public struct SlotManagerEnvironment {
    public var speech: any SpeechPlaying
    public var mic: any MicRecording
    public var images: any ImageProviding
    public var input: any InputHubbing
    public var backdrop: any BackdropSampling
    public var glassMode: GlassMode
    public var timing: BubbleTiming
    public var measurer: any TextMeasuring
    /// Monotonic seconds.
    public var now: () -> Double
    /// Creates the surface for a physical slot.
    public var makeSurface: (SlotIndex, SlotChromeModel) -> any SlotSurface
    /// `NSScreen.visibleFrame` of the screen bubbles go to right now.
    public var visibleFrame: (DisplayTarget) -> CGRect
    /// The drawing host for a Silicon (created on demand).
    public var host: (SiliconKey) -> (any DrawingHosting)?
    /// Sends an outbound request; returns whether peekd took it (and the message id of a voice message).
    public var send: (BubbleContext, BubbleOutbound) async -> BubbleDelivery
    /// `speech.done` for a send whose audio never started playing when the Carbon stopped it.
    public var speechStoppedBeforeStart: (String, Bool) -> Void
    /// Telemetry (`mac, events` / `mac, analytics`, BLUEPRINT §6.5).
    public var telemetry: (String, [String: JSONValue], PeekContext) -> Void
    /// Bare Esc for the Esc router (``CarbonEscapeKey`` in the app; inert in tests and headless runs).
    public var escapeKeys: any EscapeKeyProviding
    /// Whether one of Peek's windows is key (a typing slot panel, Settings, Simulation): Esc then arrives through
    /// AppKit, so the router lets go of the global one.
    public var keyWindowIsOpen: () -> Bool

    public init(speech: any SpeechPlaying, mic: any MicRecording, images: any ImageProviding, input: any InputHubbing,
                backdrop: any BackdropSampling, glassMode: GlassMode, timing: BubbleTiming = BubbleTiming(),
                measurer: any TextMeasuring = SystemTextMeasurer(), now: @escaping () -> Double = SlotManagerEnvironment.uptime,
                makeSurface: @escaping (SlotIndex, SlotChromeModel) -> any SlotSurface,
                visibleFrame: @escaping (DisplayTarget) -> CGRect,
                host: @escaping (SiliconKey) -> (any DrawingHosting)?,
                send: @escaping (BubbleContext, BubbleOutbound) async -> BubbleDelivery,
                speechStoppedBeforeStart: @escaping (String, Bool) -> Void = { _, _ in },
                telemetry: @escaping (String, [String: JSONValue], PeekContext) -> Void = { _, _, _ in },
                escapeKeys: any EscapeKeyProviding = InertEscapeKey(),
                keyWindowIsOpen: @escaping () -> Bool = { false }) {
        self.speech = speech
        self.mic = mic
        self.images = images
        self.input = input
        self.backdrop = backdrop
        self.glassMode = glassMode
        self.timing = timing
        self.measurer = measurer
        self.now = now
        self.makeSurface = makeSurface
        self.visibleFrame = visibleFrame
        self.host = host
        self.send = send
        self.speechStoppedBeforeStart = speechStoppedBeforeStart
        self.telemetry = telemetry
        self.escapeKeys = escapeKeys
        self.keyWindowIsOpen = keyWindowIsOpen
    }

    public nonisolated static func uptime() -> Double {
        Double(DispatchTime.now().uptimeNanoseconds) / 1e9
    }
}

/// The 8 physical slots (visual.md B1 "SlotManager"): which bubble each shows, the queue behind it,
/// the pre-warm and slide in/out, key focus, the Esc router, and everything the Carbon does in a bubble. One
/// ``BubbleMachine`` per bubble decides; this class runs its effects against the panel, the audio, the drawing and peekd.
///
/// The Esc router (peek 0.1.2, ``EscapeRouting``): bare Esc is held through ``SlotManagerEnvironment/escapeKeys`` only
/// while a new bubble is in its 3 s grace, the pointer is over a bubble, an Esc window the Carbon opened is running or a
/// popup is open, and never while one of Peek's windows is key. It is re-evaluated on every frame tick, on begin and
/// finish, when a popup opens or closes, and by a one-shot timer at the next grace or window end.
@MainActor
public final class SlotManager {
    // MARK: State

    final class Bubble {
        var machine: BubbleMachine
        let key: SiliconKey
        let slot: SlotIndex
        var timers: [BubbleTimer: Task<Void, Never>] = [:]
        var prepared: [String: PreparedImage] = [:]
        var cgImages: [String: CGImage] = [:]
        var begun = false
        var summonOnBegin = false
        /// A typing field to offer as soon as the bubble begins (a voice message that could not be transcribed).
        var typingNoticeOnBegin: String?
        /// Sends of this Silicon waiting behind this bubble (the "+N" badge; `peek.show` then `queue.state`).
        var waiting = 0
        /// The pre-warm (``BubbleEffect/prewarm``): when it began, frames ticked since, whether the drawing
        /// committed its first frame.
        var prewarmStartedAt: Double?
        var prewarmTicks = 0
        var prewarmFrameReady = false
        var prewarmTasks: [Task<Void, Never>] = []

        init(machine: BubbleMachine, key: SiliconKey, slot: SlotIndex) {
            self.machine = machine
            self.key = key
            self.slot = slot
        }

        var sendID: String? { machine.event?.sendID }
        var isPrewarming: Bool { machine.stage == .prewarming }
    }

    struct Pending {
        var source: BubbleSource
        var key: SiliconKey
        var speechAvailable: Bool
        var summoned: Bool
        /// See ``Bubble/typingNoticeOnBegin``.
        var typingNotice: String? = nil
    }

    final class PhysicalSlot {
        let index: SlotIndex
        var scheduler = SlotScheduler()
        var pending: [BubbleID: Pending] = [:]
        var active: Bubble?
        /// Sends whose audio still plays after their bubble left (single down-arrow click).
        var draining: Set<String> = []
        let chrome: SlotChromeModel
        var surface: (any SlotSurface)?
        var attachedKey: SiliconKey?

        init(index: SlotIndex, chrome: SlotChromeModel) {
            self.index = index
            self.chrome = chrome
        }
    }

    public private(set) var gate = SlotGate()
    public private(set) var mode: DisplayMode = .normal
    public private(set) var display: DisplayTarget = .main
    /// The slot table (every context) from `slots.state`.
    public private(set) var table: [SlotState] = []
    /// Sends on screen or queued, by send id (`PeekCoordinator.visible`).
    public private(set) var sends: [String: PeekShowEvent] = [:]
    public var onSendsChanged: (() -> Void)?
    /// Called whenever a bubble's phase changes (Simulation and the menu bar read it).
    public var onPhaseChanged: ((SiliconKey, Phase) -> Void)?

    private let env: SlotManagerEnvironment
    private var slots: [SlotIndex: PhysicalSlot] = [:]
    private var sequence = 0
    private var summonSerial = 0
    /// Voice messages peekd accepted and has not transcribed yet, by the message id its `voice.submit` reply named
    /// (see ``applySTT(_:)``).
    private var voiceMessages: [String: VoiceMessage] = [:]
    /// Voice messages accepted by an older peekd whose reply names no message, oldest first: matched in order.
    private var unnamedVoiceMessages: [VoiceMessage] = []
    /// Voice-message outcomes that arrived before the reply naming their message was handled (the reply and the
    /// event reach the main actor on different tasks), applied as soon as that reply is.
    private var earlyVoiceResults: [String: (result: STTResultEvent, at: Double)] = [:]
    private struct VoiceMessage {
        var slot: SlotIndex
        var key: SiliconKey
        var at: Double
    }
    /// A voice message's outcome is expected within this long (peekd's STT budget is far shorter).
    static let voiceMessageMemory: Double = 120
    /// TTS events for sends that are not showing yet (queued, or not announced), in order.
    private var ttsBuffers: [String: [TTSStreamEvent]] = [:]
    private var orphanSince: [String: Double] = [:]
    /// Sends whose audio was stopped (double click, cancel, pre-emption): late chunks are dropped.
    private var stoppedAudio: Set<String> = []
    private var backdropTones: [SiliconKey: Backdrop] = [:]
    /// Bubbles sliding out because their Silicon moved; they are queued again at the new slot.
    private var relocations: [BubbleID: SlotIndex] = [:]
    /// Sends cancelled while their bubble was still sliding out: never queued again.
    private var cancelledSends: Set<String> = []
    /// Sends whose `shown` went to peekd (once per send per process: a pre-empted bubble shown again does not resend).
    private var shownSends: Set<String> = []
    /// The newest waiting count peekd reported per send (`queue.state`), for a send not on screen yet.
    private var latestWaiting: [String: Int] = [:]
    /// Keys kept warm in the backdrop sampler (occupied slots).
    private var warmedKeys: Set<SiliconKey> = []
    private var escapeDeadline: Task<Void, Never>?
    private var escapeDeadlineAt: Double?
    /// Why Esc for peeks does not work (another app holds bare Esc), for Settings and `ui.status`.
    public private(set) var escapeProblem: String?
    public var onEscapeProblemChanged: ((String?) -> Void)?
    private let logger = PeekLogger(category: "slots")

    public init(environment: SlotManagerEnvironment) {
        env = environment
        for index in SlotIndex.allCases {
            let layout = SlotGeometry.layout(slot: index, mode: .normal, visibleFrame: environment.visibleFrame(.main))
            let slot = PhysicalSlot(index: index, chrome: SlotChromeModel(slotLayout: layout, measurer: environment.measurer))
            slot.chrome.onExpandedChanged = { [weak self] _ in self?.refreshEscape() }
            slots[index] = slot
        }
        env.escapeKeys.onPress = { [weak self] in self?.escapePressed(fromKeySlot: nil) ?? false }
        env.escapeKeys.priority = { [weak self] in
            guard let self else { return -.infinity }
            let now = self.env.now()
            return EscapeRouting.priority(self.escapeCandidates(), now: now)
        }
    }

    // MARK: Queries

    /// No bubble on screen or queued, no audio playing on after a dismissal, no recording (`app.update.prepare`).
    public var isIdle: Bool {
        slots.values.allSatisfy { $0.active == nil && $0.scheduler.queue.isEmpty && $0.draining.isEmpty } && !env.mic.isRecording
    }

    public func phase(of slot: SlotIndex) -> Phase { slots[slot]?.active?.machine.phase ?? .hidden }

    /// The bubble timings in use.
    public var timing: BubbleTiming { env.timing }

    /// The bubble machine on a slot (tests, diagnostics).
    public func machine(on slot: SlotIndex) -> BubbleMachine? { slots[slot]?.active?.machine }
    public func queue(on slot: SlotIndex) -> [SlotScheduler.Entry] { slots[slot]?.scheduler.queue ?? [] }
    public func chrome(on slot: SlotIndex) -> SlotChromeModel? { slots[slot]?.chrome }
    public func surface(on slot: SlotIndex) -> (any SlotSurface)? { slots[slot]?.surface }

    /// The Silicon occupying a physical slot: production first, then testing, then Simulation.
    public func occupant(of slot: SlotIndex) -> SlotState? {
        table.filter { $0.index == slot }.max { SlotPriority.of($0.context) < SlotPriority.of($1.context) }
    }

    // MARK: Configuration

    public func setGate(_ newGate: SlotGate) {
        guard newGate != gate else { return }
        gate = newGate
        for slot in slots.values where slot.active == nil { activateNext(slot) }
    }

    public func setMode(_ newMode: DisplayMode) {
        guard newMode != mode else { return }
        mode = newMode
        relayoutIdleSlots()
        warmBackdrops()
    }

    public func setDisplay(_ target: DisplayTarget) {
        display = target
    }

    /// The screens changed (resolution, arrangement): re-place every idle panel.
    public func screensChanged() {
        relayoutIdleSlots()
        warmBackdrops()
    }

    public func updateTable(_ newTable: [SlotState]) {
        let moves = SlotMove.between(table, newTable)
        table = newTable
        for move in moves { relocate(move) }
        // Test pills: the environment name may have changed.
        for slot in slots.values { if let bubble = slot.active { sync(bubble, in: slot) } }
        warmBackdrops()
    }

    /// Keeps an idle backdrop sample for every occupied slot, so a bubble's pill shade is known before it arrives.
    private func warmBackdrops() {
        var keys: Set<SiliconKey> = []
        for state in table {
            let key = state.siliconKey
            keys.insert(key)
            let layout = SlotGeometry.layout(slot: state.index, mode: mode, visibleFrame: env.visibleFrame(display))
            env.backdrop.warm(key, rectOnScreen: layout.visualFrameOnScreen)
        }
        for gone in warmedKeys.subtracting(keys) { env.backdrop.warm(gone, rectOnScreen: nil) }
        warmedKeys = keys
    }

    // MARK: Presenting

    /// A `peek.show`: slide in now, pre-empt or queue (see ``SlotScheduler``).
    public func present(_ event: PeekShowEvent) {
        guard let slot = slots[event.slot] else { return }
        if sends[event.sendID] != nil || slot.active?.sendID == event.sendID {
            logger.notice("ignoring a repeated peek.show for \(event.sendID)")
            return
        }
        let key = siliconKey(for: event.slot, context: event.context)
        sends[event.sendID] = event
        onSendsChanged?()
        // Sample what the bubble will sit on now, before it is activated, so its pill shade is settled when it lands.
        let layout = SlotGeometry.layout(slot: event.slot, mode: mode, visibleFrame: env.visibleFrame(display))
        env.backdrop.track(key, rectOnScreen: layout.visualFrameOnScreen)
        let pending = Pending(source: .send(event), key: key, speechAvailable: true, summoned: false)
        let id = BubbleID.send(event.sendID)
        let entry = makeEntry(id: id, pending: pending)
        let decision = slot.scheduler.decide(entry, active: activeInfo(slot), gate: gate, audioPlaying: !slot.draining.isEmpty)
        switch decision {
        case .present:
            activate(pending, id: id, in: slot)
        case .enqueue:
            enqueue(pending, entry: entry, in: slot)
            env.telemetry("send.queued", ["slot": .int(Int64(event.slot.rawValue))], event.context)
        case .replaceActive:
            var front = entry
            front.sequence = -nextSequence()
            enqueue(pending, entry: front, in: slot)
            if let active = slot.active { handle(.replaced, for: active, in: slot) }
        case .preemptActive:
            var front = entry
            front.sequence = -nextSequence()
            enqueue(pending, entry: front, in: slot)
            if let active = slot.active { handle(.preempted, for: active, in: slot) }
        }
    }

    /// `peek.cancel` (or Simulation's stop): slide out without telling peekd anything.
    public func cancel(sendID: String) {
        let id = BubbleID.send(sendID)
        for slot in slots.values {
            if let entry = slot.scheduler.remove(id) {
                slot.pending.removeValue(forKey: id)
                env.images.release(sendID: sendID)
                untrackIfIdle(entry.key)
            }
            if slot.draining.remove(sendID) != nil { stopAudio(sendID, byUser: false) }
            if let active = slot.active, active.sendID == sendID {
                cancelledSends.insert(sendID)
                handle(.cancelled, for: active, in: slot)
            }
        }
        ttsBuffers.removeValue(forKey: sendID)
        if slots.values.allSatisfy({ $0.active?.sendID != sendID }) {
            sends.removeValue(forKey: sendID)
            latestWaiting.removeValue(forKey: sendID)
            onSendsChanged?()
        }
    }

    /// `queue.state`: the waiting count behind a Silicon's current bubble changed (the "+N" badge). Applied to the
    /// bubble on screen for that send; remembered for a send that is still waiting for its slot; ignored otherwise.
    public func applyQueueState(_ state: QueueStateEvent) {
        let waiting = max(0, state.waiting)
        if let slot = slots[state.slot], let bubble = slot.active, bubble.sendID == state.sendID,
            bubble.key.context == state.context
        {
            guard bubble.waiting != waiting else { return }
            bubble.waiting = waiting
            sync(bubble, in: slot)
            return
        }
        guard let event = sends[state.sendID], event.slot == state.slot, event.context == state.context else { return }
        latestWaiting[state.sendID] = waiting
    }

    /// Stops the arrival sampling of a key with nothing left to show (a queued send was withdrawn); a bubble on screen
    /// stops it itself when it leaves.
    private func untrackIfIdle(_ key: SiliconKey) {
        let busy = slots.values.contains { slot in
            slot.active?.key == key || slot.scheduler.queue.contains { $0.key == key }
        }
        if !busy { env.backdrop.track(key, rectOnScreen: nil) }
    }

    /// A transcription outcome for a voice answer, or for a voice message (no ask).
    public func applySTT(_ result: STTResultEvent) {
        guard let askID = result.askID else {
            forgetStaleVoiceMessages()
            if let id = result.messageID, let message = voiceMessages.removeValue(forKey: id) {
                finishVoiceMessage(message, outcome: result.outcome)
            } else if !unnamedVoiceMessages.isEmpty {
                // An older peekd named no message in its reply: match in order (it transcribes in order).
                finishVoiceMessage(unnamedVoiceMessages.removeFirst(), outcome: result.outcome)
            } else if let id = result.messageID {
                // The reply naming this message has not been handled yet.
                earlyVoiceResults[id] = (result, env.now())
            } else {
                finishVoiceMessage(nil, outcome: result.outcome)
            }
            return
        }
        for slot in slots.values {
            if let active = slot.active, active.machine.event?.askID == askID {
                handle(.sttResult(result.outcome, value: result.value), for: active, in: slot)
            }
        }
    }

    private func forgetStaleVoiceMessages() {
        let now = env.now()
        voiceMessages = voiceMessages.filter { now - $0.value.at <= Self.voiceMessageMemory }
        unnamedVoiceMessages.removeAll { now - $0.at > Self.voiceMessageMemory }
        earlyVoiceResults = earlyVoiceResults.filter { now - $0.value.at <= Self.voiceMessageMemory }
    }

    /// A voice message's transcription outcome: a matched message went to its Silicon; anything else offers typing.
    private func finishVoiceMessage(_ message: VoiceMessage?, outcome: STTOutcome) {
        switch outcome {
        case .matched, .unmatched:
            break
        case .empty, .failed, .other:
            logger.notice("a voice message could not be transcribed (\(outcome.rawValue))")
            if let message {
                offerTyping(on: message.slot, for: message.key, notice: BubbleMachine.transcribeFailedNotice)
            }
        }
    }

    /// A voice message that could not be transcribed comes back as a typing field on its slot, so the Carbon can
    /// type it instead: in the bubble still up for that Silicon, or a fresh summon-style bubble. Never over another
    /// Silicon's bubble.
    private func offerTyping(on index: SlotIndex, for key: SiliconKey, notice: String) {
        guard let slot = slots[index], occupant(of: index)?.siliconKey == key else { return }
        if let active = slot.active, active.machine.stage != .leaving, active.machine.stage != .finished {
            guard active.key == key else { return }
            handle(.offerTyping(notice: notice), for: active, in: slot)
            return
        }
        summonSerial += 1
        let id = BubbleID.summon(index, serial: summonSerial)
        let pending = Pending(source: .summon(index), key: key, speechAvailable: false, summoned: true, typingNotice: notice)
        if slot.active == nil {
            activate(pending, id: id, in: slot)
        } else {
            // The bubble on the slot is still sliding out (often the one that sent the message): follow it.
            var entry = makeEntry(id: id, pending: pending)
            entry.sequence = Int.min / 2
            enqueue(pending, entry: entry, in: slot)
        }
    }

    /// The Carbon pressed a slot's hotkey (§8.5): bring its bubble forward and take key focus.
    public func summon(_ index: SlotIndex) {
        guard let slot = slots[index] else { return }
        env.telemetry("shortcut_used", ["shortcut": .string("slot_\(index.rawValue)")], occupant(of: index)?.context ?? .production)
        if let active = slot.active {
            switch active.machine.stage {
            case .prewarming, .entering, .visible:
                handle(.summoned, for: active, in: slot)
                return
            case .pending:
                active.summonOnBegin = true
                return
            case .leaving, .finished:
                break
            }
        }
        if let first = slot.scheduler.queue.first {
            slot.scheduler.summon(first.id)
            slot.pending[first.id]?.summoned = true
            // The Carbon wants this slot now: audio still playing from a dismissed bubble stops.
            if first.expectsAudio {
                for sendID in slot.draining { stopAudio(sendID, byUser: true) }
                slot.draining.removeAll()
            }
            if slot.active == nil { activateNext(slot) }
            return
        }
        guard let occupant = occupant(of: index) else { return }
        summonSerial += 1
        let id = BubbleID.summon(index, serial: summonSerial)
        let pending = Pending(source: .summon(index), key: occupant.siliconKey, speechAvailable: false, summoned: true)
        if slot.active == nil {
            activate(pending, id: id, in: slot)
        } else {
            var entry = makeEntry(id: id, pending: pending)
            entry.sequence = Int.min / 2
            enqueue(pending, entry: entry, in: slot)
        }
    }

    /// Hides everything at once (quit, pause): no requests are sent, queues are dropped.
    public func dismissAll() {
        for slot in slots.values {
            for entry in slot.scheduler.queue {
                slot.scheduler.remove(entry.id)
                if let sendID = slot.pending.removeValue(forKey: entry.id)?.source.event?.sendID {
                    env.images.release(sendID: sendID)
                }
                untrackIfIdle(entry.key)
            }
            for sendID in slot.draining { stopAudio(sendID, byUser: false) }
            slot.draining.removeAll()
            if let active = slot.active {
                if let sendID = active.sendID { cancelledSends.insert(sendID) }
                handle(.cancelled, for: active, in: slot)
            }
        }
        ttsBuffers.removeAll()
        sends = sends.filter { sendID, _ in slots.values.contains { $0.active?.sendID == sendID } }
        latestWaiting = latestWaiting.filter { sends[$0.key] != nil }
        onSendsChanged?()
    }

    // MARK: Audio

    /// Routes a TTS stream event: to the player for sends on screen (or playing on after a
    /// dismissal), into a buffer for sends still queued, nowhere for stopped ones.
    public func routeTTS(_ event: TTSStreamEvent) {
        let sendID = event.sendID
        if stoppedAudio.contains(sendID) { return }
        let playing = slots.values.contains { $0.active?.sendID == sendID || $0.draining.contains(sendID) }
        if playing {
            env.speech.handle(event)
            return
        }
        ttsBuffers[sendID, default: []].append(event)
        if orphanSince[sendID] == nil { orphanSince[sendID] = env.now() }
        dropStaleOrphans()
    }

    /// The player finished (naturally or stopped) a send's audio.
    public func speechFinished(_ finished: SpeechFinished) {
        for slot in slots.values {
            if let active = slot.active, active.sendID == finished.sendID {
                handle(.speechFinished(stoppedByUser: finished.stoppedByUser), for: active, in: slot)
            }
            if slot.draining.remove(finished.sendID) != nil, slot.active == nil { activateNext(slot) }
        }
    }

    /// Synthesis or playback failed before any audio: the bubble shows the text instead (§1.9.3).
    public func speechFailed(sendID: String) {
        for slot in slots.values {
            if let active = slot.active, active.sendID == sendID { handle(.speechFailed, for: active, in: slot) }
            if slot.draining.remove(sendID) != nil, slot.active == nil { activateNext(slot) }
        }
    }

    /// The backdrop under a bubble changed: re-shade its pills.
    public func backdropChanged(_ key: SiliconKey, _ backdrop: Backdrop) {
        backdropTones[key] = backdrop
        for slot in slots.values where slot.active?.key == key {
            slot.chrome.setShade(PillShade.forBackdrop(backdrop))
        }
    }

    /// The drawing host for `key` was replaced: attach the new one where the Silicon is on screen.
    public func hostChanged(for key: SiliconKey) {
        for slot in slots.values where slot.attachedKey == key {
            slot.attachedKey = nil
            if let bubble = slot.active, bubble.key == key, let surface = slot.surface {
                attachHost(for: key, in: slot, surface: surface)
            }
        }
    }

    /// A Silicon left its slot for good (`unregister`): forget its host attachment.
    public func forget(_ key: SiliconKey) {
        for slot in slots.values where slot.attachedKey == key && slot.active?.key != key {
            env.host(key)?.detach()
            slot.attachedKey = nil
        }
        backdropTones.removeValue(forKey: key)
    }

    // MARK: Keys

    /// A key press in a summoned panel (§8.5). Returns true when handled.
    @discardableResult
    public func handleKey(_ key: KeyInput, on index: SlotIndex) -> Bool {
        guard let slot = slots[index], let bubble = slot.active else { return false }
        let machine = bubble.machine
        let context: KeyContext =
            switch machine.input {
            case .typing: .typing(composing: key.composing)
            case .listening, .stopping: .listening
            case .transcribing: .busy
            case .none: machine.awaiting == nil ? .idle : .busy
            }
        let command = KeyClassifier.classify(
            characters: key.characters, charactersIgnoringModifiers: key.charactersIgnoringModifiers, keyCode: key.keyCode,
            commandOrControl: key.commandOrControl, context: context)
        switch command {
        case .pass:
            return false
        case .cancel:
            // Same targets as the global Esc: an open Esc window elsewhere wins, else this panel's bubble.
            escapePressed(fromKeySlot: index)
        case .voice:
            handle(.micButton, for: bubble, in: slot)
        case .type(let seed):
            handle(.startTyping(seed: seed), for: bubble, in: slot)
        case .submit:
            submitFromKeyboard(bubble, in: slot)
        case .move(let delta):
            move(delta, bubble: bubble, in: slot)
        case .toggle:
            if case .multipleChoice? = machine.askPayload?.kind, let highlight = machine.highlight {
                handle(.toggleOption(highlight), for: bubble, in: slot)
            } else {
                return false
            }
        }
        return true
    }

    private func submitFromKeyboard(_ bubble: Bubble, in slot: PhysicalSlot) {
        let machine = bubble.machine
        if machine.input == .listening {
            handle(.micButton, for: bubble, in: slot)
            return
        }
        switch machine.askPayload?.kind {
        case .singleChoice?:
            if let highlight = machine.highlight {
                handle(.chooseOption(highlight, via: .keyboard), for: bubble, in: slot)
            } else {
                handle(.submitValue(via: .keyboard), for: bubble, in: slot)
            }
        case .multipleChoice?, .slider?, .range?:
            handle(.submitValue(via: .keyboard), for: bubble, in: slot)
        case .text?:
            handle(.startTyping(seed: nil), for: bubble, in: slot)
        case nil:
            break
        }
    }

    private func move(_ delta: Int, bubble: Bubble, in slot: PhysicalSlot) {
        let machine = bubble.machine
        guard let payload = machine.askPayload else { return }
        switch payload.kind {
        case .singleChoice(let options), .multipleChoice(let options, _, _):
            guard !options.isEmpty else { return }
            let current = machine.highlight.flatMap { id in options.firstIndex { $0.id == id } }
            let next = current.map { ($0 + delta + options.count) % options.count } ?? (delta > 0 ? 0 : options.count - 1)
            handle(.setHighlight(options[next].id), for: bubble, in: slot)
        case .slider(let spec):
            let value: Double
            if case .number(let current)? = machine.askValue { value = current } else { value = spec.defaultValue }
            let next = TypedAnswerMatcher.clampAndSnap(value + Double(delta) * spec.step, min: spec.min, max: spec.max,
                                                       step: spec.step)
            handle(.setValue(.number(next)), for: bubble, in: slot)
        case .range(let spec):
            guard case .range(let lower, let upper)? = machine.askValue else { return }
            let next = TypedAnswerMatcher.clampAndSnap(upper + Double(delta) * spec.step, min: lower, max: spec.max,
                                                       step: spec.step)
            handle(.setValue(.range(lower: lower, upper: next)), for: bubble, in: slot)
        case .text:
            break
        }
    }

    // MARK: Activation

    private func nextSequence() -> Int {
        sequence += 1
        return sequence
    }

    private func makeEntry(id: BubbleID, pending: Pending) -> SlotScheduler.Entry {
        let expectsAudio = pending.speechAvailable && (pending.source.event?.speak?.status.expectsAudio ?? false)
        return SlotScheduler.Entry(id: id, key: pending.key, context: pending.key.context, expectsAudio: expectsAudio,
                                   summoned: pending.summoned, sequence: nextSequence())
    }

    private func enqueue(_ pending: Pending, entry: SlotScheduler.Entry, in slot: PhysicalSlot) {
        slot.pending[entry.id] = pending
        slot.scheduler.enqueue(entry)
    }

    private func activeInfo(_ slot: PhysicalSlot) -> SlotScheduler.Active? {
        guard let bubble = slot.active else { return nil }
        let machine = bubble.machine
        return SlotScheduler.Active(
            id: machine.id, key: bubble.key, context: bubble.key.context, isAskOpen: machine.isAskOpen,
            hasUserInput: machine.hasUserInput, isLeaving: machine.stage == .leaving || machine.stage == .finished,
            isSummon: machine.isSummon)
    }

    private func activateNext(_ slot: PhysicalSlot) {
        guard slot.active == nil,
            let entry = slot.scheduler.popNext(gate: gate, audioPlaying: !slot.draining.isEmpty),
            let pending = slot.pending.removeValue(forKey: entry.id)
        else { return }
        activate(pending, id: entry.id, in: slot)
    }

    private func activate(_ pending: Pending, id: BubbleID, in slot: PhysicalSlot) {
        let machine = BubbleMachine(id: id, source: pending.source, timing: env.timing, speechAvailable: pending.speechAvailable)
        let bubble = Bubble(machine: machine, key: pending.key, slot: slot.index)
        bubble.summonOnBegin = pending.summoned && !machine.isSummon
        bubble.typingNoticeOnBegin = pending.typingNotice
        if let event = machine.event { bubble.waiting = latestWaiting.removeValue(forKey: event.sendID) ?? event.queuedBehind }
        slot.active = bubble

        // The panel is idle now: place it for the current screen and mode.
        let layout = SlotGeometry.layout(slot: slot.index, mode: mode, visibleFrame: env.visibleFrame(display))
        let surface = surface(for: slot)
        surface.configure(layout: layout)
        // Sampled since the send arrived; tracked again in case an earlier bubble of this Silicon untracked it.
        env.backdrop.track(bubble.key, rectOnScreen: layout.visualFrameOnScreen)
        attachHost(for: bubble.key, in: slot, surface: surface)
        slot.chrome.setShade(PillShade.forBackdrop(backdropTones[bubble.key] ?? env.backdrop.backdrop(for: bubble.key)))
        slot.chrome.setContext(bubble.key.context.inputContext, tooltip: environmentTooltip(for: bubble.key),
                               glass: env.glassMode)
        sync(bubble, in: slot)

        // Audio that arrived while the send waited plays now.
        if let sendID = bubble.sendID {
            orphanSince.removeValue(forKey: sendID)
            for event in ttsBuffers.removeValue(forKey: sendID) ?? [] { env.speech.handle(event) }
        }

        guard let event = machine.event else {
            begin(bubble, in: slot)
            return
        }
        let paths = (event.show?.imagePaths ?? []) + (event.ask?.imagePaths ?? [])
        guard !paths.isEmpty else {
            begin(bubble, in: slot)
            return
        }
        Task { [weak self] in
            guard let self else { return }
            let prepared = await self.env.images.prepare(sendID: event.sendID, paths: paths)
            guard slot.active === bubble, bubble.machine.stage == .pending else { return }
            bubble.prepared = prepared
            for (path, image) in prepared {
                if let cg = self.env.images.image(for: image.handle) { bubble.cgImages[path] = cg }
            }
            self.begin(bubble, in: slot)
        }
    }

    private func begin(_ bubble: Bubble, in slot: PhysicalSlot) {
        guard !bubble.begun else { return }
        bubble.begun = true
        slot.chrome.setImages(bubble.cgImages)
        // `enter` (from .begin) comes first, then `send` with the new show / ask / speech (visual.md A3).
        handle(.begin, for: bubble, in: slot)
        if let event = bubble.machine.event, slot.active === bubble {
            env.host(bubble.key)?.deliver(.send(show: showSnapshot(event, bubble), ask: askSnapshot(event, bubble),
                                                speech: event.speak.map { InputSnapshot.Speech(text: $0.text, level: 0,
                                                                                              progress: 0, done: false) }))
        }
        if bubble.summonOnBegin { handle(.summoned, for: bubble, in: slot) }
        if let notice = bubble.typingNoticeOnBegin {
            bubble.typingNoticeOnBegin = nil
            handle(.offerTyping(notice: notice), for: bubble, in: slot)
        }
    }

    private func finish(_ bubble: Bubble, in slot: PhysicalSlot, requeue: Bool, abortedPrewarm: Bool = false) {
        for timer in bubble.timers.values { timer.cancel() }
        bubble.timers.removeAll()
        for task in bubble.prewarmTasks { task.cancel() }
        bubble.prewarmTasks.removeAll()
        // A bubble that never slid in (cancelled or pre-empted while warming up): its panel is ordered out at once.
        if abortedPrewarm { slot.surface?.slideOut() }
        slot.active = nil
        let machine = bubble.machine
        if machine.visibleSince != nil {
            let visible = machine.visibleSince.map { String(format: " after %.1f s", env.now() - $0) } ?? ""
            logger.info("close \(machine.id) at slot \(slot.index.rawValue): \(Self.describe(machine.leaveReason))\(visible)"
                + (requeue ? " (queued again)" : ""))
        }
        if let sendID = bubble.sendID {
            if machine.audioOutlivesBubble, !stoppedAudio.contains(sendID) { slot.draining.insert(sendID) }
            let cancelled = cancelledSends.remove(sendID) != nil
            if requeue, !cancelled, let event = machine.event {
                let pending = Pending(source: .send(event), key: bubble.key, speechAvailable: false, summoned: false)
                var entry = makeEntry(id: machine.id, pending: pending)
                entry.sequence = -nextSequence()
                let target = relocations.removeValue(forKey: machine.id).flatMap { slots[$0] } ?? slot
                enqueue(pending, entry: entry, in: target)
                if target !== slot, target.active == nil { activateNext(target) }
            } else {
                env.images.release(sendID: sendID)
                sends.removeValue(forKey: sendID)
                latestWaiting.removeValue(forKey: sendID)
                onSendsChanged?()
            }
            if let since = machine.visibleSince {
                env.telemetry("peek_visible", ["slot": .int(Int64(slot.index.rawValue)), "mode": .string(mode.rawValue),
                                               "visible_ms": .int(Int64(BubbleMachine.milliseconds(env.now() - since)))],
                              bubble.key.context)
            }
        }
        env.backdrop.track(bubble.key, rectOnScreen: nil)
        env.input.update(bubble.key) { state in
            state.phase = .hidden
            state.sendID = nil
            state.speakText = nil
            state.show = nil
            state.ask = nil
            state.askValue = nil
            state.askHighlight = nil
            state.typingText = nil
            state.listening = false
        }
        onPhaseChanged?(bubble.key, .hidden)
        slot.chrome.setPhase(.hidden)
        slot.chrome.setContent(ChromeContent(), askPayload: nil)
        slot.chrome.setImages([:])
        slot.chrome.resetMicLevels()
        slot.surface?.setInteraction(.none)
        if slot.chrome.slotLayout.mode != mode { relayout(slot) }
        activateNext(slot)
        refreshEscape()
    }

    // MARK: Running the machine

    private func handle(_ event: BubbleEvent, for bubble: Bubble, in slot: PhysicalSlot) {
        guard slot.active === bubble else { return }
        let before = bubble.machine.phase
        let stageBefore = bubble.machine.stage
        let speechBefore = bubble.machine.speech
        if event == .timerFired(.prewarm), stageBefore == .prewarming { logPrewarmTimeout(bubble) }
        let effects = bubble.machine.handle(event, now: env.now())
        if bubble.machine.speech != speechBefore { logSpeech(bubble.machine.speech, bubble: bubble) }
        var finished: Bool?
        for effect in effects {
            if case .finished(let requeue) = effect {
                finished = requeue
                continue
            }
            run(effect, bubble: bubble, slot: slot)
        }
        if let requeue = finished {
            finish(bubble, in: slot, requeue: requeue, abortedPrewarm: stageBefore == .prewarming)
            return
        }
        sync(bubble, in: slot)
        if bubble.machine.phase != before { onPhaseChanged?(bubble.key, bubble.machine.phase) }
        if bubble.machine.escArmedUntil != nil || stageBefore != bubble.machine.stage { refreshEscape() }
    }

    private func run(_ effect: BubbleEffect, bubble: Bubble, slot: PhysicalSlot) {
        switch effect {
        case .prewarm:
            startPrewarm(bubble, in: slot)
        case .slideIn:
            logger.info("show \(bubble.machine.id) at slot \(slot.index.rawValue): \(Self.describe(bubble.machine.event))")
            for task in bubble.prewarmTasks { task.cancel() }
            bubble.prewarmTasks.removeAll()
            let surface = surface(for: slot)
            surface.slideIn()
            env.backdrop.track(bubble.key, rectOnScreen: slot.chrome.slotLayout.visualFrameOnScreen)
        case .slideOut:
            slot.surface?.slideOut()
            env.backdrop.track(bubble.key, rectOnScreen: nil)
        case .schedule(let timer, let seconds):
            schedule(timer, after: seconds, bubble: bubble, slot: slot)
        case .cancel(let timer):
            bubble.timers.removeValue(forKey: timer)?.cancel()
        case .send(let outbound):
            send(outbound, bubble: bubble, slot: slot)
        case .stopSpeech:
            if let sendID = bubble.sendID {
                let reason = bubble.machine.leaveReason
                stopAudio(sendID, byUser: reason == .dismissed || reason == .escaped)
            }
        case .makeKey:
            surface(for: slot).setKeyFocus(true)
            if bubble.machine.input == .typing { slot.chrome.requestFieldFocus() }
        case .resignKey:
            slot.surface?.setKeyFocus(false)
        case .startRecording:
            startRecording(bubble, in: slot)
        case .stopRecording:
            Task { [weak self] in
                guard let self else { return }
                do throws(MicRecordingError) {
                    let result = try await self.env.mic.stop()
                    self.handle(.recordingFinished(result), for: bubble, in: slot)
                } catch {
                    self.handle(.recordingFailed("The recording failed: \(error.description)"), for: bubble, in: slot)
                }
            }
        case .cancelRecording:
            env.mic.cancel()
        case .drawing(let drawingEvent):
            env.host(bubble.key)?.deliver(drawingEvent)
        case .finished:
            break
        }
    }

    private func schedule(_ timer: BubbleTimer, after seconds: Double, bubble: Bubble, slot: PhysicalSlot) {
        bubble.timers.removeValue(forKey: timer)?.cancel()
        bubble.timers[timer] = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(Int((seconds * 1000).rounded())))
            guard !Task.isCancelled, let self else { return }
            bubble.timers.removeValue(forKey: timer)
            if timer == .speechStart, let sendID = bubble.sendID, self.env.speech.playback(for: sendID)?.started == true {
                self.handle(.speechStarted, for: bubble, in: slot)
                return
            }
            self.handle(.timerFired(timer), for: bubble, in: slot)
        }
    }

    private func send(_ outbound: BubbleOutbound, bubble: Bubble, slot: PhysicalSlot) {
        let context = Self.context(for: bubble, in: slot.index)
        if outbound == .shown {
            // Once per send per process: a bubble pre-empted and shown again was already reported.
            guard let sendID = context.sendID, shownSends.insert(sendID).inserted else { return }
            if shownSends.count > 2048 { shownSends = [sendID] }
        }
        switch outbound {
        case .dismissed(let gesture):
            env.telemetry("dismissed", ["gesture": .string(gesture.rawValue)], bubble.key.context)
        case .answer(_, let via):
            var data: [String: JSONValue] = ["input": .string(via.rawValue)]
            if let type = context.askType { data["ask_type"] = .string(type.rawValue) }
            if let since = bubble.machine.visibleSince {
                data["answer_latency_ms"] = .int(Int64(BubbleMachine.milliseconds(env.now() - since)))
            }
            env.telemetry("answer_submitted", data, bubble.key.context)
        default:
            break
        }
        let expectsReply: Bool =
            switch outbound {
            case .answer, .message, .voice: true
            case .dismissed, .shownDone, .focus, .shown: false
            }
        let isVoiceMessage: Bool =
            switch outbound {
            case .voice: context.askID == nil
            default: false
            }
        Task { [weak self] in
            guard let self else { return }
            let delivery = await self.env.send(context, outbound)
            if expectsReply {
                let what: String =
                    switch outbound {
                    case .answer(_, let via): "answer submitted for \(context.askID ?? "?") via \(via.rawValue)"
                    case .voice: context.askID.map { "voice answer submitted for \($0)" } ?? "voice message submitted"
                    default: "message submitted"
                    }
                if let problem = delivery.problem {
                    self.logger.notice("\(what) at slot \(slot.index.rawValue) was not taken: \(problem)")
                } else {
                    self.logger.info("\(what) at slot \(slot.index.rawValue)")
                }
            }
            self.delivered(delivery, voiceMessage: isVoiceMessage, expectsReply: expectsReply, bubble: bubble, slot: slot)
        }
    }

    private func logSpeech(_ speech: BubbleMachine.Speech, bubble: Bubble) {
        let id = bubble.machine.id
        // Speech done (with played/total ms) is logged where the player reports it (PeekCoordinator).
        switch speech {
        case .playing: logger.info("speech started for \(id)")
        case .failed: logger.info("speech unavailable for \(id); showing the text instead")
        case .none, .waiting, .finished, .stopped: break
        }
    }

    static func describe(_ reason: BubbleMachine.LeaveReason?) -> String {
        switch reason {
        case .auto(let why)?: why.rawValue
        case .dismissed?: "dismissed"
        case .escaped?: "escaped"
        case .cancelled?: "cancelled"
        case .preempted?: "preempted"
        case .replaced?: "replaced"
        case .answered?: "answered"
        case .idle?: "idle"
        case nil: "closed"
        }
    }

    static func describe(_ event: PeekShowEvent?) -> String {
        guard let event else { return "summoned by the Carbon" }
        var parts: [String] = []
        if event.speak != nil { parts.append("speak") }
        if let show = event.show { parts.append("show(\(show.elements.count))") }
        if let ask = event.ask { parts.append("ask(\(ask.type.rawValue))") }
        return parts.isEmpty ? "empty" : parts.joined(separator: " + ")
    }

    /// The request's context. Kept out of line: the Swift 6.3.1 optimizer crashes (SIL ownership verifier, in
    /// CopyPropagation) when the optional `AskPayload` borrow is inlined into ``send(_:bubble:slot:)``.
    @inline(never)
    private static func context(for bubble: Bubble, in slot: SlotIndex) -> BubbleContext {
        let event = bubble.machine.event
        var askType: AskType?
        if let ask = event?.ask { askType = ask.type }
        return BubbleContext(sendID: event?.sendID, askID: event?.askID, slot: slot, key: bubble.key, askType: askType)
    }

    /// peekd's reply to a bubble's request arrived (`problem` nil when it was taken).
    private func delivered(_ delivery: BubbleDelivery, voiceMessage: Bool, expectsReply: Bool, bubble: Bubble,
                           slot: PhysicalSlot) {
        let problem = delivery.problem
        if expectsReply { handle(.replied(ok: problem == nil, message: problem), for: bubble, in: slot) }
        guard voiceMessage, problem == nil else { return }
        forgetStaleVoiceMessages()
        let message = VoiceMessage(slot: slot.index, key: bubble.key, at: env.now())
        guard let id = delivery.messageID else {
            unnamedVoiceMessages.append(message)
            return
        }
        if let early = earlyVoiceResults.removeValue(forKey: id) {
            finishVoiceMessage(message, outcome: early.result.outcome)
        } else {
            voiceMessages[id] = message
        }
    }

    private func startRecording(_ bubble: Bubble, in slot: PhysicalSlot) {
        env.telemetry("mic_pressed", [:], bubble.key.context)
        Task { [weak self] in
            guard let self else { return }
            let mic = self.env.mic
            if mic.permission == .undetermined {
                _ = await mic.requestPermission()
            }
            guard slot.active === bubble, bubble.machine.input == .listening else { return }
            if mic.permission.isBlocked {
                self.handle(.recordingFailed(
                    "Peek can't use the microphone. Allow it in System Settings › Privacy & Security › Microphone, or type instead."),
                            for: bubble, in: slot)
                return
            }
            do throws(MicRecordingError) {
                try mic.start()
                mic.onAutoStop = { [weak self] result in
                    guard let self else { return }
                    self.handle(.recordingFinished(result), for: bubble, in: slot)
                }
                self.handle(.recordingStarted, for: bubble, in: slot)
            } catch {
                self.handle(.recordingFailed(error.description), for: bubble, in: slot)
            }
        }
    }

    private func stopAudio(_ sendID: String, byUser: Bool) {
        let started = env.speech.playback(for: sendID)?.started ?? false
        stoppedAudio.insert(sendID)
        ttsBuffers.removeValue(forKey: sendID)
        env.speech.stop(sendID: sendID)
        if !started { env.speechStoppedBeforeStart(sendID, byUser) }
        if stoppedAudio.count > 512 { stoppedAudio.removeAll() }
    }

    private func dropStaleOrphans() {
        let now = env.now()
        let known = Set(slots.values.flatMap { slot in slot.pending.values.compactMap { $0.source.event?.sendID } })
        for (sendID, since) in orphanSince where now - since > 30 && !known.contains(sendID) {
            orphanSince.removeValue(forKey: sendID)
            ttsBuffers.removeValue(forKey: sendID)
            logger.notice("dropped TTS audio for \(sendID): no bubble claimed it within 30 s")
        }
    }

    // MARK: Pre-warm

    /// Starts a bubble's pre-warm: the pill shade from the latest sample, the panel ordered in unseen with the content
    /// at rest, and the three things the slide waits for (at most ``BubbleTiming/prewarmMax``, the machine's cap):
    /// ≥ ``BubbleTiming/prewarmGlass`` and ≥ ``BubbleTiming/prewarmTicks`` frames since ordering in (the glass is
    /// live); the drawing committed a frame; a backdrop sample (≤ 2 s old with the `screen` source) or
    /// ``BubbleTiming/backdropWait`` passed.
    private func startPrewarm(_ bubble: Bubble, in slot: PhysicalSlot) {
        bubble.prewarmStartedAt = env.now()
        bubble.prewarmTicks = 0
        bubble.prewarmFrameReady = false
        slot.chrome.setShade(PillShade.forBackdrop(backdropTones[bubble.key] ?? env.backdrop.backdrop(for: bubble.key)))
        let surface = surface(for: slot)
        if surface.skipsPrewarm {
            // 0.1.1 behaviour (A/B captures only): slide in on the next turn, after the begin effects ran.
            bubble.prewarmTasks = [Task { [weak self] in
                guard !Task.isCancelled, let self, slot.active === bubble, bubble.isPrewarming else { return }
                self.handle(.prewarmed, for: bubble, in: slot)
            }]
            return
        }
        surface.prewarm()
        let timing = env.timing
        let frameTask = Task { [weak self] in
            guard let self else { return }
            let host = self.env.host(bubble.key)
            let committed = await host?.awaitFrame(timeout: .milliseconds(Int(timing.prewarmMax * 1000) + 100)) ?? true
            guard !Task.isCancelled, slot.active === bubble else { return }
            bubble.prewarmFrameReady = committed
            self.checkPrewarm(bubble, in: slot)
        }
        // The glass and backdrop waits are time based: look again when they are due, even without frame ticks.
        let recheck = Task { [weak self] in
            for delay in [timing.prewarmGlass, timing.backdropWait].sorted() where delay > 0 {
                try? await Task.sleep(for: .milliseconds(Int((delay * 1000).rounded(.up)) + 5))
                guard !Task.isCancelled, let self, slot.active === bubble else { return }
                self.checkPrewarm(bubble, in: slot)
            }
        }
        bubble.prewarmTasks = [frameTask, recheck]
        checkPrewarm(bubble, in: slot)
    }

    /// What the pre-warm still waits for (empty: slide in).
    private func prewarmMissing(_ bubble: Bubble) -> [String] {
        let timing = env.timing
        let elapsed = env.now() - (bubble.prewarmStartedAt ?? env.now())
        var missing: [String] = []
        if elapsed + 1e-6 < timing.prewarmGlass || bubble.prewarmTicks < timing.prewarmTicks { missing.append("glass") }
        if !bubble.prewarmFrameReady { missing.append("first_frame") }
        let age = env.backdrop.sampleAge(for: bubble.key)
        let sampled = age.map { env.backdrop.source == .screen ? $0 <= 2 : true } ?? false
        if !sampled, elapsed + 1e-6 < timing.backdropWait { missing.append("backdrop") }
        return missing
    }

    private func checkPrewarm(_ bubble: Bubble, in slot: PhysicalSlot) {
        guard slot.active === bubble, bubble.isPrewarming else { return }
        guard prewarmMissing(bubble).isEmpty else { return }
        handle(.prewarmed, for: bubble, in: slot)
    }

    private func logPrewarmTimeout(_ bubble: Bubble) {
        let missing = prewarmMissing(bubble)
        logger.notice("prewarm_timeout \(bubble.machine.id): sliding in after \(env.timing.prewarmMax) s without "
            + (missing.isEmpty ? "nothing" : missing.joined(separator: ", ")))
    }

    // MARK: Esc router

    func escapeCandidates() -> [EscapeRouting.Candidate] {
        slots.values.sorted { $0.index < $1.index }.compactMap { slot in
            let popupOpen = slot.chrome.expanded != nil
            guard let bubble = slot.active else {
                return popupOpen ? EscapeRouting.Candidate(slot: slot.index, popupOpen: true) : nil
            }
            let machine = bubble.machine
            return EscapeRouting.Candidate(
                slot: slot.index, popupOpen: popupOpen, onScreen: machine.stage == .entering || machine.stage == .visible,
                graceUntil: machine.graceUntil, armedUntil: machine.escArmedUntil,
                hovered: slot.surface?.pointerOverContent ?? false, slidInAt: machine.visibleSince)
        }
    }

    /// Holds or releases bare Esc for the router's current needs, and schedules the next re-evaluation.
    func refreshEscape() {
        let now = env.now()
        let candidates = escapeCandidates()
        let hold = EscapeRouting.needsEsc(candidates, now: now) && !env.keyWindowIsOpen()
        if hold != env.escapeKeys.isHeld {
            #if DEBUG
            SlideDebugLog.write(hold ? "esc-held" : "esc-released")
            #endif
            let problem = env.escapeKeys.setHeld(hold)
            if hold, problem != escapeProblem {
                escapeProblem = problem
                onEscapeProblemChanged?(problem)
            }
        }
        // One timer at the next grace or window end (kept while that deadline stays the next one: ticks call this often).
        let deadline = EscapeRouting.nextDeadline(candidates, now: now)
        guard deadline != escapeDeadlineAt else { return }
        escapeDeadline?.cancel()
        escapeDeadline = nil
        escapeDeadlineAt = deadline
        if let deadline {
            let delay = max(0, deadline - now)
            escapeDeadline = Task { [weak self] in
                try? await Task.sleep(for: .milliseconds(Int((delay * 1000).rounded(.up)) + 2))
                guard !Task.isCancelled, let self else { return }
                self.escapeDeadlineAt = nil
                self.refreshEscape()
            }
        }
    }

    /// Gives bare Esc back at once (the coordinator stops: quitting, or Simulation closing).
    public func releaseEscape() {
        escapeDeadline?.cancel()
        escapeDeadline = nil
        escapeDeadlineAt = nil
        if env.escapeKeys.isHeld { env.escapeKeys.setHeld(false) }
    }

    /// An Esc for Peek: through the global hot key (`fromKeySlot` nil) or typed into a key slot panel. Returns whether
    /// it was used.
    @discardableResult
    func escapePressed(fromKeySlot keySlot: SlotIndex?) -> Bool {
        let now = env.now()
        let candidates = escapeCandidates()
        let target = keySlot.map { EscapeRouting.keyPanelTarget(candidates, keySlot: $0, now: now) }
            ?? EscapeRouting.target(candidates, now: now)
        defer { refreshEscape() }
        switch target {
        case .popup(let index)?:
            slots[index]?.chrome.collapse()
            return true
        case .bubble(let index)?:
            guard let slot = slots[index], let bubble = slot.active else { return false }
            handle(.escape, for: bubble, in: slot)
            return true
        case nil:
            return false
        }
    }

    // MARK: Surfaces and hosts

    private func surface(for slot: PhysicalSlot) -> any SlotSurface {
        if let surface = slot.surface { return surface }
        let surface = env.makeSurface(slot.index, slot.chrome)
        let index = slot.index
        surface.onKey = { [weak self] key in self?.handleKey(key, on: index) ?? false }
        surface.onResignKey = { [weak self] in
            guard let self, let slot = self.slots[index], let bubble = slot.active else { return }
            self.handle(.resignedKey, for: bubble, in: slot)
        }
        surface.onDownArrowClick = { [weak self] count in
            guard let self, let slot = self.slots[index], let bubble = slot.active else { return }
            self.handle(count >= 2 ? .downArrowDoubleClick : .downArrowClick, for: bubble, in: slot)
        }
        surface.onVisualClick = { [weak self] unit, count in
            guard let self, let bubble = self.slots[index]?.active else { return }
            self.env.host(bubble.key)?.deliver(.click(x: unit.x, y: unit.y, count: count))
        }
        surface.onTick = { [weak self] in self?.tick(index) }
        wireActions(for: slot)
        slot.surface = surface
        return surface
    }

    private func wireActions(for slot: PhysicalSlot) {
        let index = slot.index
        var actions = slot.chrome.actions
        func withBubble(_ body: @escaping (SlotManager, Bubble, PhysicalSlot) -> Void) -> () -> Void {
            { [weak self] in
                guard let self, let slot = self.slots[index], let bubble = slot.active else { return }
                body(self, bubble, slot)
            }
        }
        actions.option = { [weak self] id in
            guard let self, let slot = self.slots[index], let bubble = slot.active else { return }
            if case .multipleChoice? = bubble.machine.askPayload?.kind {
                self.handle(.toggleOption(id), for: bubble, in: slot)
            } else {
                self.handle(.chooseOption(id, via: .click), for: bubble, in: slot)
            }
        }
        actions.setValue = { [weak self] value in
            guard let self, let slot = self.slots[index], let bubble = slot.active else { return }
            self.handle(.setValue(value), for: bubble, in: slot)
        }
        actions.submitValue = withBubble { manager, bubble, slot in manager.handle(.submitValue(via: .click), for: bubble, in: slot) }
        actions.mic = withBubble { manager, bubble, slot in manager.handle(.micButton, for: bubble, in: slot) }
        actions.keyboard = withBubble { manager, bubble, slot in
            if bubble.machine.input != .typing { manager.env.telemetry("keyboard_pressed", [:], bubble.key.context) }
            manager.handle(.keyboardButton, for: bubble, in: slot)
        }
        actions.startTyping = withBubble { manager, bubble, slot in manager.handle(.startTyping(seed: nil), for: bubble, in: slot) }
        actions.typingChanged = { [weak self] text in
            guard let self, let slot = self.slots[index], let bubble = slot.active else { return }
            self.handle(.typingChanged(text), for: bubble, in: slot)
        }
        actions.submitTyping = withBubble { manager, bubble, slot in manager.handle(.submitTyping, for: bubble, in: slot) }
        actions.hovering = { [weak self] inside in
            guard let self, let slot = self.slots[index], let bubble = slot.active else { return }
            self.handle(.pointer(inside: inside), for: bubble, in: slot)
        }
        actions.expand = withBubble { manager, bubble, slot in manager.handle(.expandAsk, for: bubble, in: slot) }
        slot.chrome.actions = actions
    }

    private func attachHost(for key: SiliconKey, in slot: PhysicalSlot, surface: any SlotSurface) {
        guard slot.attachedKey != key else { return }
        if let previous = slot.attachedKey { env.host(previous)?.detach() }
        slot.attachedKey = key
        guard let host = env.host(key) else {
            surface.visualHitTest = nil
            return
        }
        host.attach(to: surface.visualView)
        surface.visualHitTest = { [weak host] unit in host?.isOverContent(unitPoint: unit) ?? false }
    }

    private func tick(_ index: SlotIndex) {
        refreshEscapeOnTick()
        guard let slot = slots[index], let bubble = slot.active else { return }
        if bubble.isPrewarming {
            bubble.prewarmTicks += 1
            checkPrewarm(bubble, in: slot)
            return
        }
        let machine = bubble.machine
        if machine.speech == .waiting, let sendID = bubble.sendID, env.speech.playback(for: sendID)?.started == true {
            handle(.speechStarted, for: bubble, in: slot)
        }
        if machine.input == .listening || machine.input == .stopping {
            slot.chrome.pushMicLevel(env.mic.level)
        }
    }

    /// Every visible panel ticks; the router needs one look per frame, not one per panel.
    private var lastEscapeTick: Double = -1
    private func refreshEscapeOnTick() {
        let now = env.now()
        guard now - lastEscapeTick >= 0.012 || now < lastEscapeTick else { return }
        lastEscapeTick = now
        refreshEscape()
    }

    private func relayoutIdleSlots() {
        for slot in slots.values where slot.active == nil { relayout(slot) }
    }

    private func relayout(_ slot: PhysicalSlot) {
        let layout = SlotGeometry.layout(slot: slot.index, mode: mode, visibleFrame: env.visibleFrame(display))
        if let surface = slot.surface { surface.configure(layout: layout) } else { slot.chrome.setSlotLayout(layout) }
    }

    /// A Silicon moved slots (`register side`): its bubble and queued sends follow it.
    private func relocate(_ move: SlotMove) {
        guard let from = slots[move.from], let to = slots[move.to] else { return }
        for entry in from.scheduler.queue where entry.key == move.key {
            from.scheduler.remove(entry.id)
            if let pending = from.pending.removeValue(forKey: entry.id) { enqueue(pending, entry: entry, in: to) }
        }
        if let active = from.active, active.key == move.key, active.machine.event != nil {
            // Slides out here and is shown again at the new position (with its ask still pending).
            relocations[active.machine.id] = move.to
            handle(.preempted, for: active, in: from)
        }
        if from.attachedKey == move.key {
            env.host(move.key)?.detach()
            from.attachedKey = nil
        }
        if to.active == nil { activateNext(to) }
        env.host(move.key)?.deliver(.move(from: move.from, to: move.to))
    }

    // MARK: Publishing state

    private func siliconKey(for index: SlotIndex, context: PeekContext) -> SiliconKey {
        if let state = table.first(where: { $0.index == index && $0.context == context }) { return state.siliconKey }
        // peek.show arrived before slots.state: show it anyway, with the fallback visual.
        return SiliconKey(context: context, orgID: "", actorID: "")
    }

    private func environmentTooltip(for key: SiliconKey) -> String? {
        guard case .testing(let id) = key.context else { return nil }
        let info = table.first { $0.siliconKey == key }?.environment
        var parts = ["Testing environment \(info?.name ?? id)", "id \(info?.id ?? id)"]
        if let generation = info?.generation { parts.append("generation \(generation)") }
        return parts.joined(separator: " · ")
    }

    private func chromeContent(for bubble: Bubble) -> ChromeContent {
        let machine = bubble.machine
        var content = ChromeContent()
        switch bubble.key.context {
        case .testing(let id):
            content.badge = .test(name: table.first { $0.siliconKey == bubble.key }?.environment?.name ?? String(id.prefix(8)))
        case .simulation:
            content.badge = .simulation
        case .production:
            break
        }
        if let event = machine.event {
            if let show = event.show {
                content.elements = show.elements.map { element in
                    switch element {
                    case .text(let text): return .text(text)
                    case .image(let path, let caption): return .image(key: path, aspect: aspect(path, bubble), caption: caption)
                    }
                }
            } else if machine.speakAsPill, let text = event.speak?.text {
                content.elements = [.text(String(text.prefix(ShowPayload.maxTextScalars)))]
            }
            if let ask = event.ask, machine.ask != .none {
                content.question = ask.question
                switch ask.kind {
                case .text(let placeholder, _):
                    content.controls = .text(placeholder: placeholder)
                case .singleChoice(let options), .multipleChoice(let options, _, _):
                    let multiple = ask.type == .multipleChoice
                    content.controls = .choice(
                        options: options.map { option in
                            ChromeContent.Option(id: option.id, label: option.label, imageKey: option.image,
                                                 aspect: option.image.map { aspect($0, bubble) } ?? 1)
                        }, multiple: multiple)
                case .slider:
                    content.controls = .scale(isRange: false)
                case .range:
                    content.controls = .scale(isRange: true)
                }
            }
        } else if machine.input == .none {
            content.hint = "\\ to talk · type to write · esc to close"
        }
        switch machine.input {
        case .typing: content.input = .typing
        case .listening, .stopping: content.input = .listening
        case .transcribing: content.input = .transcribing
        case .none: content.input = .none
        }
        content.notice = machine.notice
        if machine.event != nil, machine.stage != .leaving { content.waiting = bubble.waiting }
        content.askCollapsed = machine.isAskCollapsed
        content.escHint = machine.escHintVisible
        return content
    }

    private func aspect(_ path: String, _ bubble: Bubble) -> CGFloat {
        guard let handle = bubble.prepared[path]?.handle, handle.height > 0 else { return 1 }
        return CGFloat(handle.width) / CGFloat(handle.height)
    }

    private func confirmEnabled(_ machine: BubbleMachine) -> Bool {
        guard machine.ask == .open, let payload = machine.askPayload else { return false }
        switch payload.kind {
        case .multipleChoice(_, let min, let max):
            if case .choices(let ids)? = machine.askValue { return (min...max).contains(ids.count) }
            return min == 0
        case .slider, .range:
            return machine.askValue != nil
        case .singleChoice:
            return machine.highlight != nil
        case .text:
            return false
        }
    }

    /// Pushes the bubble's state to its chrome, the input hub and the panel's interaction mode.
    private func sync(_ bubble: Bubble, in slot: PhysicalSlot) {
        guard slot.active === bubble else { return }
        let machine = bubble.machine
        let chrome = slot.chrome
        chrome.setContent(chromeContent(for: bubble), askPayload: machine.askPayload)
        if chrome.askValue != machine.askValue { chrome.askValue = machine.askValue }
        if chrome.highlight != machine.highlight { chrome.highlight = machine.highlight }
        chrome.setTypingText(machine.typingText)
        chrome.setPhase(machine.phase)
        chrome.setConfirmEnabled(confirmEnabled(machine))
        if machine.input != .listening && machine.input != .stopping { chrome.resetMicLevels() }

        let interaction: SlotInteraction =
            switch machine.stage {
            case .visible: .full
            case .entering: machine.summoned ? .full : .none
            case .leaving: .downArrowOnly
            case .pending, .prewarming, .finished: .none
            }
        slot.surface?.setInteraction(interaction)

        let layout = chrome.slotLayout
        let event = machine.event
        let speakText: String? =
            switch machine.speech {
            case .waiting, .playing, .finished: event?.speak?.text
            case .none, .failed, .stopped: nil
            }
        env.input.update(bubble.key) { state in
            state.slot = slot.index
            state.mode = layout.mode
            state.phase = machine.phase
            state.context = bubble.key.context.inputContext
            state.glass = env.glassMode
            state.visualFrameOnScreen = layout.visualFrameOnScreen
            state.facing = layout.facing
            state.sendID = event?.sendID
            state.speakText = speakText
            state.show = event?.show
            state.ask = machine.ask == .none ? nil : event?.ask
            state.askValue = machine.askValue
            state.askHighlight = machine.highlight
            state.typingText = machine.input == .typing ? machine.typingText : nil
            state.listening = machine.input == .listening
        }
    }

    // MARK: Drawing snapshots

    private func showSnapshot(_ event: PeekShowEvent, _ bubble: Bubble) -> InputSnapshot.Show? {
        guard let show = event.show else { return nil }
        return InputSnapshot.Show(elements: show.elements.compactMap { element in
            switch element {
            case .text(let text):
                return .text(text)
            case .image(let path, let caption):
                guard let prepared = bubble.prepared[path] else { return nil }
                return .image(prepared.handle, caption: caption, colors: prepared.colors)
            }
        })
    }

    private func askSnapshot(_ event: PeekShowEvent, _ bubble: Bubble) -> InputSnapshot.Ask? {
        guard let ask = event.ask else { return nil }
        return InputSnapshot.Ask(ask, value: bubble.machine.askValue, highlight: nil, images: bubble.prepared)
    }
}
