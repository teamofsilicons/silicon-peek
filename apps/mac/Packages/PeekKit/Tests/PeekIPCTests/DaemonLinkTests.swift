import Darwin
import Foundation
import PeekCore
import Testing

@testable import PeekIPC

private func configuration(_ server: FakePeekd, requestTimeout: Duration = .seconds(3)) -> DaemonLink.Configuration {
    DaemonLink.Configuration(
        socketPath: server.socketPath, appBuild: 1000, appVersion: "0.1.0", helloTimeout: .seconds(2),
        requestTimeout: requestTimeout,
        backoff: DaemonLink.Backoff(initial: .milliseconds(20), maximum: .milliseconds(80), jitter: 0))
}

private struct Timeout: Error {}

/// Waits for the first state satisfying `predicate`.
@discardableResult
private func waitForState(_ link: DaemonLink, timeout: Duration = .seconds(5),
                          _ predicate: @escaping @Sendable (DaemonLinkState) -> Bool) async throws -> DaemonLinkState {
    let states = await link.states()
    return try await withThrowingTaskGroup(of: DaemonLinkState?.self) { group in
        group.addTask {
            for await state in states where predicate(state) { return state }
            return nil
        }
        group.addTask {
            try await Task.sleep(for: timeout)
            return nil
        }
        defer { group.cancelAll() }
        guard let first = try await group.next(), let state = first else { throw Timeout() }
        return state
    }
}

/// Collects `count` events or throws after `timeout`.
private func collect(_ events: AsyncStream<DaemonEvent>, count: Int, timeout: Duration = .seconds(5)) async throws -> [DaemonEvent] {
    try await withThrowingTaskGroup(of: [DaemonEvent]?.self) { group in
        group.addTask {
            var collected: [DaemonEvent] = []
            for await event in events {
                collected.append(event)
                if collected.count == count { return collected }
            }
            return collected
        }
        group.addTask {
            try await Task.sleep(for: timeout)
            return nil
        }
        defer { group.cancelAll() }
        guard let first = try await group.next(), let events = first else { throw Timeout() }
        return events
    }
}

@Suite("DaemonLink", .serialized)
struct DaemonLinkTests {
    @Test("hello (role ui) is the first frame and a good reply connects")
    func handshake() async throws {
        let server = try FakePeekd()
        defer { server.shutdown() }
        let link = DaemonLink(configuration: configuration(server))
        await link.start()

        let connection = try await server.nextConnection()
        let hello = try await connection.nextFrame()
        let id = try #require(hello["id"]?.stringValue)
        #expect(hello.fields == ["v": 1, "id": .string(id), "op": "hello", "role": "ui", "app_build": 1000,
                                 "app_version": "0.1.0", "protocols": [1]])
        try connection.write(FrameCoding.okReply(id: id, result: ["protocol": 1, "peekd_version": "0.1.0-test", "extra": 1]))
        let state = try await waitForState(link) { $0.isConnected }
        #expect(state == .connected(HelloResult(protocolVersion: 1, peekdVersion: "0.1.0-test")))
        await link.stop()
        #expect(await link.currentState == .stopped)
    }

    @Test("replies are matched to requests by id, in any order; error replies throw .remote")
    func correlation() async throws {
        let server = try FakePeekd()
        defer { server.shutdown() }
        let link = DaemonLink(configuration: configuration(server))
        await link.start()
        let connection = try await server.acceptHandshake()
        try await waitForState(link) { $0.isConnected }

        async let first = link.send(FocusRequest(slot: .top))
        async let second = link.send(MessageRequest(slot: .left, text: "hello"))
        let a = try await connection.nextFrame()
        let b = try await connection.nextFrame()
        let focus = a["op"] == "focus" ? a : b
        let message = a["op"] == "focus" ? b : a
        #expect(message["text"] == "hello")
        // Answer the message first, then fail the focus.
        try connection.write(FrameCoding.okReply(id: message["id"]!.stringValue!, result: ["delivered": true]))
        try connection.write(FrameCoding.errorReply(
            id: focus["id"]!.stringValue!,
            error: IPCErrorBody(code: "side_not_registered", message: "no Silicon at position 1", hint: "wait", retryable: false)))

        let messageReply = try await second
        #expect(messageReply.result == ["delivered": true])
        do {
            _ = try await first
            Issue.record("focus should have failed")
        } catch let error as DaemonLinkError {
            guard case .remote(let op, let body) = error else {
                Issue.record("expected .remote, got \(error)")
                return
            }
            #expect(op == "focus")
            #expect(body.code == "side_not_registered")
        }
        await link.stop()
    }

    @Test("a request without a reply times out; a late reply is ignored")
    func timeout() async throws {
        let server = try FakePeekd()
        defer { server.shutdown() }
        let link = DaemonLink(configuration: configuration(server))
        await link.start()
        let connection = try await server.acceptHandshake()
        try await waitForState(link) { $0.isConnected }

        let clock = ContinuousClock()
        let started = clock.now
        await #expect(throws: DaemonLinkError.timedOut(op: "focus", after: .milliseconds(150))) {
            try await link.send(FocusRequest(slot: .top), blobs: [], timeout: .milliseconds(150))
        }
        #expect(clock.now - started < .seconds(2))
        let request = try await connection.nextFrame()
        try connection.write(FrameCoding.okReply(id: request["id"]!.stringValue!, result: [:]))
        // The connection survives and keeps working.
        async let next = link.send(FocusRequest(slot: .right))
        let frame = try await connection.nextFrame()
        try connection.write(FrameCoding.okReply(id: frame["id"]!.stringValue!, result: [:]))
        _ = try await next
        await link.stop()
    }

    @Test("events arrive decoded and in order, blobs included")
    func events() async throws {
        let server = try FakePeekd()
        defer { server.shutdown() }
        let link = DaemonLink(configuration: configuration(server))
        let stream = await link.events()
        await link.start()
        let connection = try await server.acceptHandshake()

        let slots = SlotsStateEvent(slots: [SlotState(index: .bottom, actorID: "si:dj", orgID: "tos")])
        let pcm = Data((0..<4800).map { UInt8(truncatingIfNeeded: $0) })
        try connection.write(DaemonEvent.slotsState(slots).frame())
        try connection.write(DaemonEvent.ttsBegin(TTSBegin(sendID: "snd_1", estFrames: 2400)).frame())
        try connection.write(DaemonEvent.ttsChunk(TTSChunk(sendID: "snd_1", seq: 0, pcm: pcm)).frame())
        try connection.write(Frame(fields: ["v": 1, "event": "future.thing", "x": 1]))
        try connection.write(DaemonEvent.ttsEnd(TTSEnd(sendID: "snd_1", totalFrames: 2400)).frame())

        let received = try await collect(stream, count: 5)
        #expect(received == [
            .slotsState(slots),
            .ttsBegin(TTSBegin(sendID: "snd_1", estFrames: 2400)),
            .ttsChunk(TTSChunk(sendID: "snd_1", seq: 0, pcm: pcm)),
            .unknown(name: "future.thing", fields: ["x": 1]),
            .ttsEnd(TTSEnd(sendID: "snd_1", totalFrames: 2400)),
        ])
        await link.stop()
    }

    @Test("peekd → UI requests reach the handler and its reply goes back with blobs")
    func incomingRequests() async throws {
        let server = try FakePeekd()
        defer { server.shutdown() }
        let link = DaemonLink(configuration: configuration(server))
        await link.setRequestHandler { request in
            switch request {
            case .drawingValidate(let validate):
                return .validation(ValidationReport(
                    ok: true, stats: ValidationStats(frames: 90, p50Ms: 0.3, p95Ms: 0.5, maxMs: 0.9, opsMax: 212, glassRebuilds: 1),
                    logs: [validate.scriptPath], previewPNG: validate.preview ? Data([0x89, 0x50, 0x4E, 0x47]) : nil))
            case .appUpdatePrepare(let build):
                return .encode(ReadyResult(ready: build.build > 1000))
            default:
                return .failure(IPCErrorBody(code: "not_handled", message: "test handler"))
            }
        }
        await link.start()
        let connection = try await server.acceptHandshake()

        try connection.write(Frame(fields: ["v": 1, "id": "r1", "op": "drawing.validate", "script_path": "/tmp/cassette.js",
                                            "preview": true]))
        let validateReply = try await connection.nextFrame()
        #expect(validateReply["id"] == "r1")
        #expect(validateReply["ok"] == true)
        #expect(validateReply["result"]?["ok"] == true)
        #expect(validateReply["result"]?["stats"]?["frames"] == 90)
        #expect(validateReply["result"]?["logs"] == ["/tmp/cassette.js"])
        #expect(validateReply.blobs == [Data([0x89, 0x50, 0x4E, 0x47])])

        try connection.write(Frame(fields: ["v": 1, "id": "r2", "op": "app.update.prepare", "build": 1001]))
        let prepareReply = try await connection.nextFrame()
        #expect(prepareReply["result"] == ["ready": true])

        try connection.write(Frame(fields: ["v": 1, "id": "r3", "op": "drawing.teleport"]))
        let unknownReply = try await connection.nextFrame()
        #expect(unknownReply["ok"] == false)
        #expect(unknownReply["error"]?["code"] == "unknown_op")

        try connection.write(Frame(fields: ["v": 1, "id": "r4", "op": "drawing.load", "context": "production"]))
        let invalidReply = try await connection.nextFrame()
        #expect(invalidReply["error"]?["code"] == "invalid_request")
        await link.stop()
    }

    @Test("without a handler every peekd request is refused with unknown_op")
    func noHandler() async throws {
        let server = try FakePeekd()
        defer { server.shutdown() }
        let link = DaemonLink(configuration: configuration(server))
        await link.start()
        let connection = try await server.acceptHandshake()
        try connection.write(Frame(fields: ["v": 1, "id": "q", "op": "app.quit", "build": 1001]))
        let reply = try await connection.nextFrame()
        #expect(reply["id"] == "q")
        #expect(reply["error"]?["code"] == "unknown_op")
        await link.stop()
    }

    @Test("a dropped connection fails pending requests, then the link reconnects and works again")
    func reconnect() async throws {
        let server = try FakePeekd()
        defer { server.shutdown() }
        let link = DaemonLink(configuration: configuration(server))
        await link.start()
        let first = try await server.acceptHandshake()
        try await waitForState(link) { $0.isConnected }

        let pending = Task { try await link.send(FocusRequest(slot: .top)) }
        _ = try await first.nextFrame()
        first.close()
        await #expect(throws: DaemonLinkError.disconnected(op: "focus")) { try await pending.value }

        let second = try await server.acceptHandshake()
        try await waitForState(link) { $0.isConnected }
        async let again = link.send(FocusRequest(slot: .left))
        let frame = try await second.nextFrame()
        try second.write(FrameCoding.okReply(id: frame["id"]!.stringValue!, result: [:]))
        _ = try await again
        #expect(server.acceptedCount == 2)
        await link.stop()
    }

    @Test("a frame with duplicate keys is a protocol violation: the link drops it and reconnects")
    func strictJSON() async throws {
        let server = try FakePeekd()
        defer { server.shutdown() }
        let link = DaemonLink(configuration: configuration(server))
        await link.start()
        let first = try await server.acceptHandshake()
        try await waitForState(link) { $0.isConnected }
        try first.writeRaw(Data(Array(#"{"v":1,"event":"peek.cancel","send_id":"a","send_id":"b","reason":"expired"}"#.utf8) + [0x0A]))
        #expect(await first.waitForClientClose())
        _ = try await server.acceptHandshake()
        try await waitForState(link) { $0.isConnected }
        await link.stop()
    }

    @Test("sending before start or after stop fails with .notConnected")
    func notConnected() async throws {
        let link = DaemonLink(configuration: DaemonLink.Configuration(socketPath: "/nonexistent/peekd.sock", appBuild: 1, appVersion: "0"))
        do {
            _ = try await link.send(FocusRequest(slot: .top))
            Issue.record("expected .notConnected")
        } catch {
            guard case .notConnected(let reason) = error else {
                Issue.record("expected .notConnected, got \(error)")
                return
            }
            #expect(reason.contains("start()"))
        }
    }

    @Test("voice.submit sends the WAV blob byte for byte; an oversized WAV is refused before sending")
    func voiceSubmit() async throws {
        let server = try FakePeekd()
        defer { server.shutdown() }
        let link = DaemonLink(configuration: configuration(server))
        await link.start()
        let connection = try await server.acceptHandshake()
        try await waitForState(link) { $0.isConnected }

        let wav = Data((0..<100_000).map { UInt8(truncatingIfNeeded: $0 &* 7) })
        async let submitted = link.send(VoiceSubmitRequest(sendID: "snd_1", askID: "ask_1", slot: .bottom, durationMs: 3125),
                                        blobs: [wav], timeout: nil)
        let frame = try await connection.nextFrame()
        #expect(frame["op"] == "voice.submit")
        #expect(frame["duration_ms"] == 3125)
        #expect(frame.blobs == [wav])
        try connection.write(FrameCoding.okReply(id: frame["id"]!.stringValue!, result: [:]))
        #expect(try await submitted.messageID == nil, "an older peekd's empty reply names no message")

        // A voice message: peekd's reply names it.
        async let message = link.send(VoiceSubmitRequest(sendID: nil, askID: nil, slot: .bottom, durationMs: 900),
                                      blobs: [wav], timeout: nil)
        let messageFrame = try await connection.nextFrame()
        #expect(messageFrame["ask_id"] == .null)
        try connection.write(FrameCoding.okReply(id: messageFrame["id"]!.stringValue!, result: ["message_id": "cmsg_7"]))
        #expect(try await message.messageID == "cmsg_7")

        let oversized = Data(count: FrameLimits.maxWAVBytes + 1)
        do {
            _ = try await link.send(VoiceSubmitRequest(sendID: nil, askID: nil, slot: .top, durationMs: 1), blobs: [oversized],
                                    timeout: nil)
            Issue.record("expected .invalidRequest")
        } catch {
            guard case .invalidRequest(let op, let reason) = error else {
                Issue.record("expected .invalidRequest, got \(error)")
                return
            }
            #expect(op == "voice.submit")
            #expect(reason.contains("\(FrameLimits.maxWAVBytes)"))
        }
        // Nothing was sent for the refused request: the next frame is the next request.
        async let focus = link.send(FocusRequest(slot: .top))
        let next = try await connection.nextFrame()
        #expect(next["op"] == "focus")
        try connection.write(FrameCoding.okReply(id: next["id"]!.stringValue!, result: [:]))
        _ = try await focus
        await link.stop()
    }

    @Test("a refused hello keeps retrying with a precise reason until a good hello")
    func helloRefused() async throws {
        let server = try FakePeekd()
        defer { server.shutdown() }
        let link = DaemonLink(configuration: configuration(server))
        await link.start()
        let first = try await server.nextConnection()
        let hello = try await first.nextFrame()
        try first.write(FrameCoding.errorReply(
            id: hello["id"]!.stringValue!,
            error: IPCErrorBody(code: "ui_identity_mismatch", message: "peer is not Peek.app", retryable: true)))
        let waiting = try await waitForState(link) {
            if case .waiting(_, let reason) = $0 { return reason.contains("ui_identity_mismatch") }
            return false
        }
        if case .waiting(let delay, _) = waiting { #expect(delay <= .milliseconds(80)) }
        _ = try await server.acceptHandshake()
        try await waitForState(link) { $0.isConnected }
        await link.stop()
    }

    @Test("a connection that drops during the handshake is retried at once, not after the hello timeout")
    func dropDuringHandshake() async throws {
        let server = try FakePeekd()
        defer { server.shutdown() }
        let link = DaemonLink(configuration: configuration(server))
        await link.start()
        let first = try await server.nextConnection()
        _ = try await first.nextFrame()
        let clock = ContinuousClock()
        let dropped = clock.now
        first.close()
        _ = try await server.acceptHandshake()
        try await waitForState(link) { $0.isConnected }
        #expect(clock.now - dropped < .seconds(1))
        await link.stop()
    }

    @Test("a daemon speaking another protocol major is not accepted")
    func protocolMismatch() async throws {
        let server = try FakePeekd()
        defer { server.shutdown() }
        let link = DaemonLink(configuration: configuration(server))
        await link.start()
        _ = try await server.acceptHandshake(result: ["protocol": 2, "peekd_version": "9.0.0"])
        try await waitForState(link) {
            if case .waiting(_, let reason) = $0 { return reason.contains("speaks protocol 2") }
            return false
        }
        await link.stop()
    }

    @Test("stop fails pending requests and finishes event streams")
    func stop() async throws {
        let server = try FakePeekd()
        defer { server.shutdown() }
        let link = DaemonLink(configuration: configuration(server))
        let events = await link.events()
        await link.start()
        let connection = try await server.acceptHandshake()
        try await waitForState(link) { $0.isConnected }
        let pending = Task { try await link.send(FocusRequest(slot: .top)) }
        _ = try await connection.nextFrame()
        await link.stop()
        await #expect(throws: DaemonLinkError.disconnected(op: "focus")) { try await pending.value }
        var iterator = events.makeAsyncIterator()
        #expect(await iterator.next() == nil)
    }

    @Test("an insecure socket directory is refused before connecting")
    func insecureDirectory() async throws {
        let server = try FakePeekd(directoryMode: 0o755)
        defer { server.shutdown() }
        let link = DaemonLink(configuration: configuration(server))
        await link.start()
        try await waitForState(link) {
            if case .waiting(_, let reason) = $0 { return reason.contains("mode is 755") }
            return false
        }
        #expect(server.acceptedCount == 0)
        await link.stop()
    }
}

@Suite("Socket path and backoff")
struct SocketPathTests {
    @Test("the default path is /var/tmp/silicon-peek-<uid>/peekd.sock; PEEK_DAEMON_SOCKET overrides it")
    func paths() {
        #expect(DaemonSocket.defaultPath(uid: 501) == "/var/tmp/silicon-peek-501/peekd.sock")
        #expect(DaemonSocket.defaultPath(uid: 501).utf8.count == 36)
        #expect(DaemonSocket.resolvePath(environment: [:], uid: 501) == "/var/tmp/silicon-peek-501/peekd.sock")
        #expect(DaemonSocket.resolvePath(environment: ["PEEK_DAEMON_SOCKET": "/tmp/t.sock"], uid: 501) == "/tmp/t.sock")
        #expect(DaemonSocket.resolvePath(environment: ["PEEK_DAEMON_SOCKET": ""], uid: 501) == "/var/tmp/silicon-peek-501/peekd.sock")
    }

    @Test("connect errors say what is wrong and what to do")
    func connectErrors() throws {
        let long = "/tmp/" + String(repeating: "x", count: 120) + "/peekd.sock"
        #expect(throws: SocketConnectError.pathTooLong(path: long, bytes: long.utf8.count)) {
            try UnixSocket.connect(path: long)
        }

        var template = Array((NSTemporaryDirectory() + "pk.XXXXXX").utf8CString)
        let directory = String(cString: try #require(mkdtemp(&template)))
        defer {
            unlink(directory + "/file")
            rmdir(directory)
        }
        chmod(directory, 0o700)
        #expect(throws: SocketConnectError.directoryMissing(path: directory + "/missing")) {
            try UnixSocket.connect(path: directory + "/missing/peekd.sock")
        }
        #expect(throws: SocketConnectError.socketMissing(path: directory + "/peekd.sock")) {
            try UnixSocket.connect(path: directory + "/peekd.sock")
        }
        FileManager.default.createFile(atPath: directory + "/file", contents: Data())
        #expect(throws: SocketConnectError.notASocket(path: directory + "/file")) {
            try UnixSocket.connect(path: directory + "/file")
        }
        #expect(SocketConnectError.socketMissing(path: "/x").description.contains("peek daemon restart"))
    }

    @Test("backoff doubles from 250 ms to a 4 s cap with bounded jitter")
    func backoff() {
        let backoff = DaemonLink.Backoff()
        #expect((1...7).map { backoff.delay(attempt: $0, random: 0) }
            == [.milliseconds(250), .milliseconds(500), .seconds(1), .seconds(2), .seconds(4), .seconds(4), .seconds(4)])
        #expect(backoff.delay(attempt: 1, random: 1) == .milliseconds(300))
        #expect(backoff.delay(attempt: 1, random: -1) == .milliseconds(200))
    }
}
