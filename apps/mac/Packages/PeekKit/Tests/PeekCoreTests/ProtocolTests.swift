import Foundation
import Testing

@testable import PeekCore

/// Wire-shape tests against the BLUEPRINT §1.6 tables.
@Suite("IPC protocol v1")
struct ProtocolTests {
    private func header<R: UIRequest>(_ request: R, blobs: [Data] = []) throws -> JSONValue {
        let frame = try FrameCoding.request(id: "id-1", op: R.op, payload: request, blobs: blobs)
        let data = try frame.encoded()
        let line = data.prefix(while: { $0 != 0x0A })
        return try StrictJSON.parse(Data(line))
    }

    private func frame(_ json: String, blobs: [Data] = []) throws -> Frame {
        guard case .object(let fields) = try StrictJSON.parse(json) else { throw FrameCoding.CodingFailure("not an object") }
        return Frame(fields: fields, blobs: blobs)
    }

    // MARK: UI → peekd

    @Test("hello (role ui) matches §1.6")
    func hello() throws {
        #expect(try header(HelloRequest(appBuild: 1000, appVersion: "0.1.0"))
            == ["v": 1, "id": "id-1", "op": "hello", "role": "ui", "app_build": 1000, "app_version": "0.1.0",
                "protocols": [1]])
        let result = try FrameCoding.decode(HelloResult.self, from: ["protocol": 1, "peekd_version": "0.1.0", "extra": true])
        #expect(result == HelloResult(protocolVersion: 1, peekdVersion: "0.1.0"))
    }

    @Test("answer carries raw values for every ask type")
    func answer() throws {
        #expect(try header(AnswerRequest(sendID: "snd_1", askID: "ask_1", value: .choice("keep"), via: .click))
            == ["v": 1, "id": "id-1", "op": "answer", "send_id": "snd_1", "ask_id": "ask_1", "value": "keep", "via": "click"])
        #expect(try header(AnswerRequest(sendID: "s", askID: "a", value: .choices(["a", "b"]), via: .keyboard))["value"]
            == ["a", "b"])
        #expect(try header(AnswerRequest(sendID: "s", askID: "a", value: .range(lower: 2, upper: 7.5), via: .click))["value"]
            == [2, 7.5])
        #expect(try header(AnswerRequest(sendID: "s", askID: "a", value: .number(0.25), via: .click))["value"] == 0.25)
    }

    @Test("voice.submit keeps explicit nulls, carries the WAV as a blob, and has a 4 MiB limit")
    func voiceSubmit() throws {
        let request = VoiceSubmitRequest(sendID: nil, askID: nil, slot: .bottom, durationMs: 3200)
        let frame = try FrameCoding.request(id: "id-1", op: VoiceSubmitRequest.op, payload: request, blobs: [Data([1, 2])])
        #expect(frame.fields["send_id"] == .null)
        #expect(frame.fields["ask_id"] == .null)
        #expect(frame.fields["slot"] == 5)
        #expect(frame.fields["duration_ms"] == 3200)
        #expect(frame.fields["context"] == nil)
        #expect(frame.blobs == [Data([1, 2])])
        #expect(VoiceSubmitRequest.maxBlobBytes == 4 << 20)
        #expect(FocusRequest.maxBlobBytes == FrameLimits.maxBlobBytes)

        // The reply names a voice message; an older peekd's empty reply (and unknown fields) still decode.
        #expect(try FrameCoding.decode(VoiceSubmitReply.self, from: ["message_id": "cmsg_1"]).messageID == "cmsg_1")
        #expect(try FrameCoding.decode(VoiceSubmitReply.self, from: ["message_id": .null]).messageID == nil)
        #expect(try FrameCoding.decode(VoiceSubmitReply.self, from: ["simulated": true]).messageID == nil)

        let withContext = VoiceSubmitRequest(sendID: "snd_1", askID: "ask_1", slot: .top, durationMs: 1,
                                             context: .testing(environmentID: "3f2c"), languages: ["en-US", "de"])
        let fields = try header(withContext)
        #expect(fields["context"] == "3f2c")
        #expect(fields["languages"] == ["en-US", "de"])
    }

    @Test("the remaining UI requests match §1.6 field names")
    func otherRequests() throws {
        #expect(try header(MessageRequest(slot: .right, text: "hi"))
            == ["v": 1, "id": "id-1", "op": "message", "slot": 3, "text": "hi", "via": "keyboard"])
        #expect(try header(DismissedRequest(sendID: "s", gesture: .downArrowDouble))["gesture"] == "down_arrow_double")
        #expect(try header(SpeechDoneRequest(sendID: "s", stoppedByUser: true, playedMs: 1200, totalMs: 4000))
            == ["v": 1, "id": "id-1", "op": "speech.done", "send_id": "s", "stopped_by_user": true, "played_ms": 1200,
                "total_ms": 4000])
        #expect(try header(ShownDoneRequest(sendID: "s", visibleMs: 5000, reason: .speechDone))["reason"] == "speech_done")
        #expect(try header(FocusRequest(slot: .topLeft)) == ["v": 1, "id": "id-1", "op": "focus", "slot": 8])
        #expect(try header(DrawingErrorRequest(context: .production, orgID: "tos", actorID: "si:dj", reason: .throwsRepeatedly,
                                               message: "TypeError", stack: nil))
            == ["v": 1, "id": "id-1", "op": "drawing.error", "context": "production", "org_id": "tos", "actor_id": "si:dj",
                "reason": "throws", "message": "TypeError", "stack": .null])
        let telemetry = try header(TelemetryRequest(events: [TelemetryEvent(id: "evt_1", type: "glass_mode", data: ["mode": "live"])]))
        #expect(telemetry["events"] == [["id": "evt_1", "type": "glass_mode", "data": ["mode": "live"]]])
        #expect(try header(SettingsChangedRequest(key: .showTestPeeks, value: false))
            == ["v": 1, "id": "id-1", "op": "settings.changed", "key": "show_test_peeks", "value": false])
    }

    @Test("ops are the §1.6 names")
    func opNames() {
        #expect(HelloRequest.op == "hello")
        #expect(AnswerRequest.op == "answer")
        #expect(VoiceSubmitRequest.op == "voice.submit")
        #expect(MessageRequest.op == "message")
        #expect(DismissedRequest.op == "dismissed")
        #expect(SpeechDoneRequest.op == "speech.done")
        #expect(ShownDoneRequest.op == "shown.done")
        #expect(FocusRequest.op == "focus")
        #expect(DrawingErrorRequest.op == "drawing.error")
        #expect(TelemetryRequest.op == "telemetry")
        #expect(SettingsChangedRequest.op == "settings.changed")
        #expect(DrawingValidateRequest.op == "drawing.validate")
        #expect(DrawingLoadRequest.op == "drawing.load")
        #expect(DaemonRequest.appUpdatePrepareOp == "app.update.prepare")
        #expect(DaemonRequest.appQuitOp == "app.quit")
    }

    // MARK: Envelopes

    @Test("envelopes classify requests, replies and events; others are invalid")
    func envelopes() throws {
        if case .request(let id, let op, _) = try Envelope(frame: frame(#"{"v":1,"id":"a","op":"app.quit","build":1001}"#)) {
            #expect(id == "a")
            #expect(op == "app.quit")
        } else {
            Issue.record("expected a request")
        }
        #expect(try Envelope(frame: frame(#"{"v":1,"id":"b","ok":true,"result":{"x":1}}"#))
            == .reply(id: "b", outcome: .ok(["x": 1]), frame: frame(#"{"v":1,"id":"b","ok":true,"result":{"x":1}}"#)))
        let failure = try Envelope(frame: frame(
            #"{"v":1,"id":"c","ok":false,"error":{"code":"side_taken","message":"m","hint":"h","retryable":false,"details":{"free":[1,4]}}}"#))
        guard case .reply(_, .failure(let body), _) = failure else {
            Issue.record("expected an error reply")
            return
        }
        #expect(body == IPCErrorBody(code: "side_taken", message: "m", hint: "h", retryable: false, details: ["free": [1, 4]]))

        #expect(throws: Envelope.Invalid.self) { try Envelope(frame: frame(#"{"v":2,"event":"x"}"#)) }
        #expect(throws: Envelope.Invalid.self) { try Envelope(frame: frame(#"{"event":"x"}"#)) }
        #expect(throws: Envelope.Invalid.self) { try Envelope(frame: frame(#"{"v":1,"id":"z"}"#)) }
        #expect(throws: Envelope.Invalid.self) { try Envelope(frame: frame(#"{"v":1,"id":"z","ok":false}"#)) }
    }

    @Test("replies built by FrameCoding have the envelope shape")
    func replyFrames() throws {
        let ok = try DaemonReply.encode(ReadyResult(ready: true)).frame(id: "r1").encoded()
        #expect(String(decoding: ok, as: UTF8.self) == #"{"id":"r1","ok":true,"result":{"ready":true},"v":1}"# + "\n")
        let failure = DaemonReply.failure(.unknownOp("drawing.frobnicate")).frame(id: "r2")
        #expect(failure.fields["ok"] == false)
        #expect(failure.fields["error"]?["code"] == "unknown_op")
        #expect(failure.fields["error"]?["message"]?.stringValue?.contains("drawing.frobnicate") == true)
    }

    // MARK: peekd → UI events

    @Test("slots.state decodes, defaulting display name and initial")
    func slotsState() throws {
        let event = DaemonEvent(name: "slots.state", frame: try frame(#"""
            {"v":1,"event":"slots.state","slots":[
              {"index":5,"context":"production","actor_id":"si:dj","org_id":"tos","display_name":"DJ","initial":"D",
               "drawing":{"sha256":"ab","path":"/p/ab.js"},"hotkey":true},
              {"index":3,"context":"0192f1c2-0000-7000-8000-000000000000","actor_id":"si:cleanup","org_id":"tos",
               "drawing":null,"hotkey":false,"future_field":1}]}
            """#))
        guard case .slotsState(let state) = event else {
            Issue.record("expected slots.state, got \(event)")
            return
        }
        #expect(state.slots.count == 2)
        #expect(state.slots[0] == SlotState(index: .bottom, actorID: "si:dj", orgID: "tos", displayName: "DJ", initial: "D",
                                            drawing: DrawingRef(sha256: "ab", path: "/p/ab.js")))
        #expect(state.slots[1].context == .testing(environmentID: "0192f1c2-0000-7000-8000-000000000000"))
        #expect(state.slots[1].displayName == "cleanup")
        #expect(state.slots[1].initial == "C")
        #expect(state.slots[1].hotkey == false)
    }

    @Test("peek.show decodes speak, show and ask with the ask_id")
    func peekShow() throws {
        let event = DaemonEvent(name: "peek.show", frame: try frame(#"""
            {"v":1,"event":"peek.show","send_id":"snd_1","ask_id":"ask_1","slot":5,"context":"production",
             "speak":{"text":"Keep it?","status":"pending"},"show":null,
             "ask":{"question":"Delete old.zip?","type":"single_choice","options":[{"id":"keep","label":"Keep"},
                    {"id":"delete","label":"Delete","image":"/cache/images/aa.png"}]},
             "duration_ms":null,"queued_behind":0}
            """#))
        guard case .peekShow(let show) = event else {
            Issue.record("expected peek.show, got \(event)")
            return
        }
        #expect(show.sendID == "snd_1")
        #expect(show.askID == "ask_1")
        #expect(show.slot == .bottom)
        #expect(show.speak == SpeakInfo(text: "Keep it?", status: .pending))
        #expect(show.ask?.options == [AskOption(id: "keep", label: "Keep"),
                                      AskOption(id: "delete", label: "Delete", image: "/cache/images/aa.png")])
        #expect(show.ask?.imagePaths == ["/cache/images/aa.png"])

        // Re-encoding and decoding gives the same value.
        let reencoded = DaemonEvent(name: "peek.show", frame: try DaemonEvent.peekShow(show).frame())
        #expect(reencoded == .peekShow(show))
    }

    @Test("peek.show falls back to ask.ask_id and defaults context and queue")
    func peekShowFallbacks() throws {
        let event = DaemonEvent(name: "peek.show", frame: try frame(#"""
            {"v":1,"event":"peek.show","send_id":"snd_2","slot":1,
             "show":{"elements":[{"type":"text","text":"Now playing"},{"type":"image","path":"/c/co2.jpg","caption":"CO2"}]},
             "ask":null,"speak":{"text":"hola","status":"unsupported_language"}}
            """#))
        guard case .peekShow(let show) = event else {
            Issue.record("expected peek.show, got \(event)")
            return
        }
        #expect(show.askID == nil)
        #expect(show.context == .production)
        #expect(show.queuedBehind == 0)
        #expect(show.show?.elements == [.text("Now playing"), .image(path: "/c/co2.jpg", caption: "CO2")])
        #expect(show.speak?.status == .unsupportedLanguage)
        #expect(show.speak?.status.expectsAudio == false)

        let nested = DaemonEvent(name: "peek.show", frame: try frame(#"""
            {"v":1,"event":"peek.show","send_id":"s","slot":2,"ask":{"ask_id":"ask_9","question":"Q?","type":"text"}}
            """#))
        guard case .peekShow(let nestedShow) = nested else {
            Issue.record("expected peek.show")
            return
        }
        #expect(nestedShow.askID == "ask_9")
    }

    @Test("tts.* events decode; a chunk must carry exactly one blob of at most 64 KiB")
    func tts() throws {
        #expect(DaemonEvent(name: "tts.begin", frame: try frame(
            #"{"v":1,"event":"tts.begin","send_id":"s","format":"s16le","sample_rate":24000,"channels":1,"est_frames":48000}"#))
            == .ttsBegin(TTSBegin(sendID: "s", estFrames: 48000)))
        let pcm = Data(repeating: 7, count: 960)
        #expect(DaemonEvent(name: "tts.chunk", frame: try frame(#"{"v":1,"event":"tts.chunk","send_id":"s","seq":3}"#, blobs: [pcm]))
            == .ttsChunk(TTSChunk(sendID: "s", seq: 3, pcm: pcm)))
        if case .malformed = DaemonEvent(name: "tts.chunk", frame: try frame(#"{"v":1,"event":"tts.chunk","send_id":"s","seq":3}"#)) {
        } else {
            Issue.record("a chunk without a blob must be malformed")
        }
        let oversized = try frame(#"{"v":1,"event":"tts.chunk","send_id":"s","seq":1}"#, blobs: [Data(count: 64 * 1024 + 1)])
        if case .malformed = DaemonEvent(name: "tts.chunk", frame: oversized) {
        } else {
            Issue.record("an oversized chunk must be malformed")
        }
        #expect(DaemonEvent(name: "tts.end", frame: try frame(#"{"v":1,"event":"tts.end","send_id":"s","total_frames":47000}"#))
            == .ttsEnd(TTSEnd(sendID: "s", totalFrames: 47000)))
        #expect(DaemonEvent(name: "tts.error", frame: try frame(
            #"{"v":1,"event":"tts.error","send_id":"s","error":{"code":"speech_unavailable","message":"m","retryable":true}}"#))
            == .ttsError(TTSErrorEvent(sendID: "s", error: IPCErrorBody(code: "speech_unavailable", message: "m", retryable: true))))
    }

    @Test("peek.cancel, stt.result and restarting decode; unknown values are kept")
    func otherEvents() throws {
        #expect(DaemonEvent(name: "peek.cancel", frame: try frame(#"{"v":1,"event":"peek.cancel","send_id":"s","reason":"expired"}"#))
            == .peekCancel(PeekCancelEvent(sendID: "s", reason: .expired)))
        #expect(DaemonEvent(name: "peek.cancel", frame: try frame(#"{"v":1,"event":"peek.cancel","send_id":"s","reason":"new_reason"}"#))
            == .peekCancel(PeekCancelEvent(sendID: "s", reason: .other("new_reason"))))
        #expect(DaemonEvent(name: "stt.result", frame: try frame(
            #"{"v":1,"event":"stt.result","ask_id":"a","message_id":null,"outcome":"matched","value":["x","y"],"error":null}"#))
            == .sttResult(STTResultEvent(askID: "a", messageID: nil, outcome: .matched, value: ["x", "y"])))
        #expect(DaemonEvent(name: "restarting", frame: try frame(#"{"v":1,"event":"restarting","to_build":1001}"#))
            == .restarting(RestartingEvent(toBuild: 1001)))
        #expect(DaemonEvent(name: "brand.new", frame: try frame(#"{"v":1,"event":"brand.new","a":1}"#))
            == .unknown(name: "brand.new", fields: ["a": 1]))
        if case .malformed(let name, let reason) = DaemonEvent(name: "tts.end", frame: try frame(#"{"v":1,"event":"tts.end"}"#)) {
            #expect(name == "tts.end")
            #expect(reason.contains("send_id"))
        } else {
            Issue.record("expected malformed")
        }
    }

    @Test("no live word or transcript data exists anywhere in the protocol")
    func noWordEvents() {
        // D9: speech.word, the word event and mic.transcript are gone.
        #expect(DaemonEvent(name: "speech.word", frame: Frame(fields: ["v": 1, "event": "speech.word"])).name == "speech.word")
        if case .unknown = DaemonEvent(name: "word", frame: Frame(fields: ["v": 1, "event": "word"])) {
        } else {
            Issue.record("a word event must not be understood")
        }
    }

    // MARK: peekd → UI requests

    @Test("peekd requests decode; bad fields give invalid_request; unknown ops are kept")
    func daemonRequests() throws {
        #expect(DaemonRequest.decode(op: "drawing.validate", frame: try frame(
            #"{"v":1,"id":"1","op":"drawing.validate","script_path":"/tmp/x.js","preview":true,"dump_frame":30}"#))
            == .success(.drawingValidate(DrawingValidateRequest(scriptPath: "/tmp/x.js", preview: true, dumpFrame: 30))))
        #expect(DaemonRequest.decode(op: "drawing.validate", frame: try frame(
            #"{"v":1,"id":"1","op":"drawing.validate","script_path":"/tmp/x.js"}"#))
            == .success(.drawingValidate(DrawingValidateRequest(scriptPath: "/tmp/x.js"))))
        #expect(DaemonRequest.decode(op: "drawing.load", frame: try frame(
            #"{"v":1,"id":"1","op":"drawing.load","context":"production","org_id":"tos","actor_id":"si:dj","slot":5,"script_path":"/d.js","sha256":"ab"}"#))
            == .success(.drawingLoad(DrawingLoadRequest(context: .production, orgID: "tos", actorID: "si:dj", slot: .bottom,
                                                        scriptPath: "/d.js", sha256: "ab"))))
        #expect(DaemonRequest.decode(op: "app.update.prepare", frame: try frame(#"{"v":1,"id":"1","op":"app.update.prepare","build":1001}"#))
            == .success(.appUpdatePrepare(AppBuildRequest(build: 1001))))
        #expect(DaemonRequest.decode(op: "app.quit", frame: try frame(#"{"v":1,"id":"1","op":"app.quit","build":1001}"#))
            == .success(.appQuit(AppBuildRequest(build: 1001))))
        // peekd relays `peek doctor` as op "doctor" (silicon-peek-client UiDoctor); the UI answers it like ui.status.
        #expect(DaemonRequest.decode(op: "doctor", frame: try frame(#"{"v":1,"id":"1","op":"doctor"}"#)) == .success(.uiStatus))
        #expect(DaemonRequest.decode(op: "ui.status", frame: try frame(#"{"v":1,"id":"1","op":"ui.status"}"#)) == .success(.uiStatus))
        #expect(DaemonRequest.decode(op: "x.y", frame: try frame(#"{"v":1,"id":"1","op":"x.y","k":2}"#))
            == .success(.unknown(op: "x.y", fields: ["k": 2])))
        guard case .failure(let error) = DaemonRequest.decode(op: "drawing.load", frame: try frame(#"{"v":1,"id":"1","op":"drawing.load"}"#)) else {
            Issue.record("expected invalid_request")
            return
        }
        #expect(error.code == "invalid_request")
        #expect(error.message.contains("drawing.load"))
    }

    @Test("a validation reply carries the report and the preview PNG as bin")
    func validationReply() throws {
        let report = ValidationReport(
            ok: false,
            stats: ValidationStats(frames: 90, p50Ms: 0.31, p95Ms: 0.58, maxMs: 0.92, opsMax: 212, glassRebuilds: 1),
            warnings: [ValidationWarning(code: "glass_outline_unstable", message: "glass fill #1 changed outline in 90/90 frames")],
            logs: ["hello"], error: ValidationFailure(message: "TypeError: x", stack: "at cassette.js:18:34", frame: 14,
                                                      inputSummary: "phase=showing, show=null"),
            previewPNG: Data([0x89, 0x50]))
        let frame = DaemonReply.validation(report).frame(id: "v1")
        #expect(frame.blobs == [Data([0x89, 0x50])])
        let result = try #require(frame.fields["result"])
        #expect(result["ok"] == false)
        #expect(result["stats"]?["p95_ms"] == 0.58)
        #expect(result["error"]?["input_summary"] == "phase=showing, show=null")
        #expect(result["previewPNG"] == nil)
        let decoded = try FrameCoding.decode(ValidationReport.self, from: result)
        var expected = report
        expected.previewPNG = nil
        #expect(decoded == expected)
    }
}
