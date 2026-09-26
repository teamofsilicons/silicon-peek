import Foundation
import PeekAudio
import PeekCore
import PeekDrawing
import PeekInput
import Testing

@testable import PeekUI

/// An in-memory `DaemonLinking`: records requests, lets tests push events and peekd requests.
actor FakeLink: DaemonLinking {
    private(set) var sent: [(op: String, fields: JSONValue)] = []
    private var eventContinuations: [AsyncStream<DaemonEvent>.Continuation] = []
    private var stateContinuations: [AsyncStream<DaemonLinkState>.Continuation] = []
    private var handler: (@Sendable (DaemonRequest) async -> DaemonReply)?
    private(set) var started = false

    func start() {
        started = true
        for continuation in stateContinuations {
            continuation.yield(.connected(HelloResult(protocolVersion: 1, peekdVersion: "fake")))
        }
    }

    func stop() {
        for continuation in eventContinuations { continuation.finish() }
        for continuation in stateContinuations { continuation.finish() }
    }

    func events() -> AsyncStream<DaemonEvent> {
        let (stream, continuation) = AsyncStream.makeStream(of: DaemonEvent.self)
        eventContinuations.append(continuation)
        return stream
    }

    func states() -> AsyncStream<DaemonLinkState> {
        let (stream, continuation) = AsyncStream.makeStream(of: DaemonLinkState.self)
        continuation.yield(.idle)
        stateContinuations.append(continuation)
        return stream
    }

    func send<R: UIRequest>(_ request: R, blobs: [Data], timeout: Duration?) async throws(DaemonLinkError) -> R.Reply {
        let fields: JSONValue
        do {
            fields = .object(try FrameCoding.fields(of: request))
        } catch {
            throw .invalidRequest(op: R.op, reason: error.description)
        }
        sent.append((R.op, fields))
        do {
            return try FrameCoding.decode(R.Reply.self, from: [:])
        } catch {
            throw .invalidReply(op: R.op, reason: error.description)
        }
    }

    func setRequestHandler(_ handler: (@Sendable (DaemonRequest) async -> DaemonReply)?) {
        self.handler = handler
    }

    func push(_ event: DaemonEvent) {
        for continuation in eventContinuations { continuation.yield(event) }
    }

    func request(_ request: DaemonRequest) async -> DaemonReply? {
        guard let handler else { return nil }
        return await handler(request)
    }

    func sentOps() -> [String] { sent.map(\.op) }
}

@Suite("PeekUI coordinator (placeholder)")
@MainActor
struct CoordinatorTests {
    private func makeCoordinator(_ link: FakeLink) -> PeekCoordinator {
        let home = FileManager.default.temporaryDirectory.appendingPathComponent("peek-ui-\(UUID().uuidString)")
        let paths = PeekPaths(home: home)
        let images = ImageCache(paths: paths)
        let speech = SpeechPlayer()
        let mic = MicRecorder()
        let backdrop = BackdropSampler(paths: paths)
        return PeekCoordinator(
            link: link, drawing: DrawingRuntime(), speech: speech, mic: mic, images: images,
            input: InputHub(images: images, speech: speech, mic: mic, backdrop: backdrop), backdrop: backdrop,
            paths: paths, persistSettings: false)
    }

    /// Polls until `condition` holds (events hop through AsyncStreams and tasks).
    private func eventually(_ condition: () -> Bool, timeout: Duration = .seconds(3)) async -> Bool {
        let deadline = ContinuousClock.now + timeout
        while ContinuousClock.now < deadline {
            if condition() { return true }
            try? await Task.sleep(for: .milliseconds(5))
        }
        return condition()
    }

    @Test("start subscribes, connects and routes slots, shows and cancels")
    func routesEvents() async {
        let link = FakeLink()
        let coordinator = makeCoordinator(link)
        await coordinator.start()
        #expect(await link.started)
        #expect(await eventually { coordinator.linkState.isConnected })

        await link.push(.slotsState(SlotsStateEvent(slots: [
            SlotState(index: .bottom, actorID: "si:dj", orgID: "tos"), SlotState(index: .top, actorID: "si:a", orgID: "tos"),
        ])))
        #expect(await eventually { coordinator.slots.map(\.index) == [.top, .bottom] })

        let ask = PeekShowEvent(sendID: "snd_1", askID: "ask_1", slot: .bottom,
                                ask: AskPayload(question: "Keep?", kind: .text(placeholder: nil, maxLength: 500)))
        await link.push(.peekShow(ask))
        #expect(await eventually { coordinator.visible["snd_1"] != nil })
        #expect(!coordinator.isIdleForUpdate)

        await link.push(.sttResult(STTResultEvent(askID: "ask_1", messageID: nil, outcome: .matched, value: "yes")))
        #expect(await eventually { coordinator.visible.isEmpty })
        #expect(coordinator.isIdleForUpdate)

        await link.push(.peekShow(PeekShowEvent(sendID: "snd_2", slot: .top, show: ShowPayload(elements: [.text("hi")]),
                                                durationMs: 30)))
        #expect(await eventually { coordinator.visible["snd_2"] != nil })
        #expect(await eventually { coordinator.visible["snd_2"] == nil })
        await coordinator.stop()
    }

    @Test("a show without an ask slides out after its duration and reports shown.done")
    func showExpires() async {
        let link = FakeLink()
        let coordinator = makeCoordinator(link)
        await coordinator.start()
        coordinator.present(PeekShowEvent(sendID: "snd_3", slot: .top, show: ShowPayload(elements: [.text("hi")]), durationMs: 20))
        #expect(coordinator.visible["snd_3"] != nil)
        let reported = await eventually {
            coordinator.visible.isEmpty
        }
        #expect(reported)
        var ops: [String] = []
        for _ in 0..<200 {
            ops = await link.sentOps()
            if ops.contains("shown.done") { break }
            try? await Task.sleep(for: .milliseconds(5))
        }
        #expect(ops.contains("shown.done"))
        await coordinator.stop()
    }

    @Test("peekd requests: update readiness follows idleness; unknown ops are refused; validation answers")
    func answersRequests() async throws {
        let link = FakeLink()
        let coordinator = makeCoordinator(link)
        await coordinator.start()

        #expect(await link.request(.appUpdatePrepare(AppBuildRequest(build: 1001))) == .encode(ReadyResult(ready: true)))
        coordinator.present(PeekShowEvent(sendID: "s", askID: "a", slot: .top,
                                          ask: AskPayload(question: "Q?", kind: .text(placeholder: nil, maxLength: 10))))
        #expect(await link.request(.appUpdatePrepare(AppBuildRequest(build: 1001))) == .encode(ReadyResult(ready: false)))
        #expect(await link.request(.appQuit(AppBuildRequest(build: 1001))) == .encode(ReadyResult(ready: false)))
        #expect(await link.request(.unknown(op: "x", fields: [:])) == .failure(.unknownOp("x")))

        let reply = try #require(await link.request(.drawingValidate(DrawingValidateRequest(scriptPath: "/nonexistent.js"))))
        guard case .ok(let result, _) = reply else {
            Issue.record("expected an ok reply carrying the report, got \(reply)")
            return
        }
        #expect(result["ok"] == false)
        await coordinator.stop()
    }

    @Test("settings changes apply locally and are sent as settings.changed")
    func settings() async {
        let link = FakeLink()
        let coordinator = makeCoordinator(link)
        await coordinator.start()
        coordinator.setSetting(.mode, "compact")
        #expect(coordinator.settings.mode == .compact)
        coordinator.setSetting(.mode, "sideways")
        #expect(coordinator.settings.mode == .compact)
        #expect(coordinator.lastProblem?.contains("mode") == true)
        var ops: [String] = []
        for _ in 0..<200 {
            ops = await link.sentOps()
            if ops.contains("settings.changed") { break }
            try? await Task.sleep(for: .milliseconds(5))
        }
        #expect(ops == ["settings.changed"])
        await coordinator.stop()
    }

    @Test("presence changes reach peekd in order as presence requests")
    func presence() async {
        let link = FakeLink()
        let coordinator = makeCoordinator(link)
        await coordinator.start()
        #expect(coordinator.carbonPresence == .available)
        coordinator.presenceChanged(PresenceRequest(available: false, reason: .locked))
        coordinator.presenceChanged(PresenceRequest(available: false, reason: .asleep))
        coordinator.presenceChanged(.available)
        #expect(coordinator.carbonPresence == .available)
        var sent: [JSONValue] = []
        for _ in 0..<200 {
            sent = await link.sent.filter { $0.op == "presence" }.map(\.fields)
            if sent.count == 3 { break }
            try? await Task.sleep(for: .milliseconds(5))
        }
        #expect(sent == [["available": false, "reason": "locked"], ["available": false, "reason": "asleep"],
                         ["available": true, "reason": "ok"]])
        await coordinator.stop()
    }

    @Test("a drawing that switches to the fallback while on screen is reported to peekd as drawing.error")
    func drawingErrorReachesPeekd() async {
        let link = FakeLink()
        let coordinator = makeCoordinator(link)
        await coordinator.start()
        let key = SiliconKey(context: .production, orgID: "tos", actorID: "si:dj")
        let host = coordinator.host(for: key)
        host.onFailure?(DrawingFailure(reason: .throwsRepeatedly,
                                       message: "frame() threw 10 times in a row; last error: Error: boom",
                                       stack: "at crash.js:3:9"))
        var sent: [JSONValue] = []
        for _ in 0..<200 {
            sent = await link.sent.filter { $0.op == "drawing.error" }.map(\.fields)
            if !sent.isEmpty { break }
            try? await Task.sleep(for: .milliseconds(5))
        }
        #expect(sent == [["context": "production", "org_id": "tos", "actor_id": "si:dj", "reason": "throws",
                          "message": "frame() threw 10 times in a row; last error: Error: boom",
                          "stack": "at crash.js:3:9"]])
        await coordinator.stop()
    }
}

@Suite("Carbon presence")
@MainActor
struct PresenceTests {
    @Test("locked or switched-away sessions are 'locked'; sleeping displays 'asleep'; no display 'display_off'")
    func mapping() {
        #expect(PresenceState().request == .available)
        #expect(PresenceState(screenLocked: true).request == PresenceRequest(available: false, reason: .locked))
        #expect(PresenceState(sessionInactive: true).request == PresenceRequest(available: false, reason: .locked))
        #expect(PresenceState(displaysAsleep: true).request == PresenceRequest(available: false, reason: .asleep))
        #expect(PresenceState(noDisplays: true).request == PresenceRequest(available: false, reason: .displayOff))
        // A lock wins: the display waking on the lock screen does not make the Carbon available.
        #expect(PresenceState(screenLocked: true, displaysAsleep: true).request.reason == .locked)
    }

    @Test("the monitor reports each change once: lock, display sleep, wake while locked, unlock")
    func monitorReportsChanges() {
        var reports: [PresenceRequest] = []
        let monitor = PresenceMonitor { reports.append($0) }
        monitor.update { $0.screenLocked = true }
        monitor.update { $0.displaysAsleep = true }
        monitor.update { $0.displaysAsleep = false }
        monitor.update { $0.screenLocked = false }
        monitor.update { $0.screenLocked = false }
        #expect(reports == [PresenceRequest(available: false, reason: .locked), .available])
        monitor.update { $0.displaysAsleep = true }
        #expect(reports.last == PresenceRequest(available: false, reason: .asleep))
    }
}
