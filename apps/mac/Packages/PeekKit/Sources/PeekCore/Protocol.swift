import Foundation

// IPC protocol v1 between Peek.app (role "ui") and peekd, mirroring BLUEPRINT §1.6.
//
//   request  {"v":1,"id":"<uuid>","op":"<name>", ...fields}
//   reply    {"v":1,"id":"<uuid>","ok":true,"result":{...}}
//            {"v":1,"id":"<uuid>","ok":false,"error":{"code","message","hint","retryable","details"}}
//   event    {"v":1,"event":"<name>", ...fields}
//
// Unknown fields are ignored and additive fields are allowed. Unknown ops are
// refused by name with `unknown_op`. Field names are snake_case on the wire;
// every type below spells its CodingKeys out so nothing depends on a key strategy.

public enum IPCProtocol {
    /// The envelope version, `"v"`.
    public static let envelopeVersion: Int64 = 1
    /// Protocol majors this build speaks, sent in `hello.protocols`.
    public static let supportedProtocols = [1]
    /// Default request timeout (CLI requests use the same 30 s).
    public static let defaultRequestTimeout: Duration = .seconds(30)
}

// MARK: - Errors

/// The error object of a failed reply, identical to the CLI error shape (§7.5).
public struct IPCErrorBody: Codable, Sendable, Equatable, Error, CustomStringConvertible {
    public var code: String
    public var message: String
    public var hint: String?
    public var retryable: Bool
    public var details: JSONValue?

    public init(code: String, message: String, hint: String? = nil, retryable: Bool = false, details: JSONValue? = nil) {
        self.code = code
        self.message = message
        self.hint = hint
        self.retryable = retryable
        self.details = details
    }

    enum CodingKeys: String, CodingKey { case code, message, hint, retryable, details }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        code = try c.decode(String.self, forKey: .code)
        message = try c.decode(String.self, forKey: .message)
        hint = try c.decodeIfPresent(String.self, forKey: .hint)
        retryable = try c.decodeIfPresent(Bool.self, forKey: .retryable) ?? false
        details = try c.decodeIfPresent(JSONValue.self, forKey: .details)
    }

    public var description: String {
        var text = "\(code): \(message)"
        if let hint { text += " (hint: \(hint))" }
        return text
    }

    public static func unknownOp(_ op: String) -> IPCErrorBody {
        IPCErrorBody(
            code: "unknown_op",
            message: "Peek.app does not implement the IPC op \"\(op)\" (protocol \(IPCProtocol.supportedProtocols.map(String.init).joined(separator: ",")))",
            hint: "update Peek.app, or stop sending this op to a UI of this build")
    }

    public static func invalidRequest(_ op: String, _ why: String) -> IPCErrorBody {
        IPCErrorBody(code: "invalid_request", message: "IPC op \"\(op)\" has invalid fields: \(why)")
    }
}

// MARK: - Envelope

/// A frame classified by its envelope.
public enum Envelope: Sendable, Equatable {
    case request(id: String, op: String, frame: Frame)
    case reply(id: String, outcome: ReplyOutcome, frame: Frame)
    case event(name: String, frame: Frame)

    public enum ReplyOutcome: Sendable, Equatable {
        case ok(JSONValue)
        case failure(IPCErrorBody)
    }

    /// Why a frame is not a valid v1 envelope.
    public enum Invalid: Error, Equatable, Sendable, CustomStringConvertible {
        case unsupportedVersion(JSONValue?)
        case unclassifiable
        case badReply(String)

        public var description: String {
            switch self {
            case .unsupportedVersion(let v):
                return "frame has envelope version \(v?.jsonString ?? "(missing)"); this build speaks v1"
            case .unclassifiable:
                return "frame is neither a request (id+op), a reply (id+ok) nor an event (event)"
            case .badReply(let why):
                return "reply is malformed: \(why)"
            }
        }
    }

    public init(frame: Frame) throws(Invalid) {
        guard case .int(IPCProtocol.envelopeVersion) = frame["v"] else {
            throw .unsupportedVersion(frame["v"])
        }
        if case .string(let name) = frame["event"] {
            self = .event(name: name, frame: frame)
            return
        }
        guard case .string(let id) = frame["id"] else { throw .unclassifiable }
        if case .string(let op) = frame["op"] {
            self = .request(id: id, op: op, frame: frame)
            return
        }
        guard case .bool(let ok) = frame["ok"] else { throw .unclassifiable }
        if ok {
            self = .reply(id: id, outcome: .ok(frame["result"] ?? .object([:])), frame: frame)
        } else {
            guard let error = frame["error"] else { throw .badReply("ok:false without an error object") }
            do {
                let body = try FrameCoding.decode(IPCErrorBody.self, from: error)
                self = .reply(id: id, outcome: .failure(body), frame: frame)
            } catch {
                throw .badReply("error object: \(error)")
            }
        }
    }

    public var frame: Frame {
        switch self {
        case .request(_, _, let frame), .reply(_, _, let frame), .event(_, let frame): return frame
        }
    }
}

/// Encoding and decoding between Codable payloads and frames.
public enum FrameCoding {
    /// Why a payload could not be turned into a frame or back.
    public struct CodingFailure: Error, Sendable, CustomStringConvertible {
        public let description: String
        public init(_ description: String) { self.description = description }
    }

    /// Encodes a Codable payload into a JSON object's fields.
    public static func fields<T: Encodable>(of payload: T) throws(CodingFailure) -> [String: JSONValue] {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        encoder.nonConformingFloatEncodingStrategy = .throw
        let data: Data
        do {
            data = try encoder.encode(payload)
        } catch {
            throw CodingFailure("cannot encode \(T.self): \(error)")
        }
        let value: JSONValue
        do {
            value = try StrictJSON.parse(data)
        } catch {
            throw CodingFailure("encoded \(T.self) is not strict JSON: \(error)")
        }
        guard case .object(let fields) = value else {
            throw CodingFailure("\(T.self) must encode to a JSON object")
        }
        return fields
    }

    /// Decodes a Codable type from a JSON value.
    public static func decode<T: Decodable>(_ type: T.Type, from value: JSONValue) throws(CodingFailure) -> T {
        do {
            return try JSONDecoder().decode(T.self, from: value.serialized())
        } catch let error as DecodingError {
            throw CodingFailure("cannot decode \(T.self): \(DecodingErrorDescription(error))")
        } catch {
            throw CodingFailure("cannot decode \(T.self): \(error)")
        }
    }

    public static func request<R: Encodable>(id: String, op: String, payload: R, blobs: [Data] = [])
        throws(CodingFailure) -> Frame
    {
        var fields = try Self.fields(of: payload)
        fields["v"] = .int(IPCProtocol.envelopeVersion)
        fields["id"] = .string(id)
        fields["op"] = .string(op)
        return Frame(fields: fields, blobs: blobs)
    }

    public static func okReply(id: String, result: JSONValue, blobs: [Data] = []) -> Frame {
        Frame(
            fields: ["v": .int(IPCProtocol.envelopeVersion), "id": .string(id), "ok": .bool(true), "result": result],
            blobs: blobs)
    }

    public static func okReply<T: Encodable>(id: String, payload: T, blobs: [Data] = []) throws(CodingFailure) -> Frame {
        okReply(id: id, result: .object(try fields(of: payload)), blobs: blobs)
    }

    public static func errorReply(id: String, error: IPCErrorBody) -> Frame {
        let body: JSONValue
        do {
            body = .object(try fields(of: error))
        } catch {
            body = .object(["code": .string("internal_error"), "message": .string("\(error)"), "retryable": .bool(false)])
        }
        return Frame(
            fields: ["v": .int(IPCProtocol.envelopeVersion), "id": .string(id), "ok": .bool(false), "error": body])
    }

    public static func event<E: Encodable>(name: String, payload: E, blobs: [Data] = []) throws(CodingFailure) -> Frame {
        var fields = try Self.fields(of: payload)
        fields["v"] = .int(IPCProtocol.envelopeVersion)
        fields["event"] = .string(name)
        return Frame(fields: fields, blobs: blobs)
    }
}

/// A readable one-line rendering of a `DecodingError` (the default description is a wall of text).
public struct DecodingErrorDescription: CustomStringConvertible, Sendable {
    public let description: String

    public init(_ error: DecodingError) {
        func path(_ context: DecodingError.Context) -> String {
            let keys = context.codingPath.map { key in key.intValue.map { "[\($0)]" } ?? key.stringValue }
            return keys.isEmpty ? "(root)" : keys.joined(separator: ".")
        }
        switch error {
        case .keyNotFound(let key, let context):
            description = "missing field \"\(key.stringValue)\" at \(path(context))"
        case .typeMismatch(let type, let context):
            description = "field \(path(context)) has the wrong type (expected \(type)): \(context.debugDescription)"
        case .valueNotFound(let type, let context):
            description = "field \(path(context)) is null but must be \(type)"
        case .dataCorrupted(let context):
            description = "field \(path(context)) is invalid: \(context.debugDescription)"
        @unknown default:
            description = "\(error)"
        }
    }
}

// MARK: - UI → peekd requests

/// A request Peek.app sends to peekd. `op` is the wire name; `Reply` decodes `result`.
public protocol UIRequest: Encodable, Sendable {
    associatedtype Reply: Decodable & Sendable
    static var op: String { get }
    /// Largest blob this request may carry (checked before anything is sent).
    static var maxBlobBytes: Int { get }
}

extension UIRequest {
    public static var maxBlobBytes: Int { FrameLimits.maxBlobBytes }
}

/// A reply whose `result` carries nothing the UI needs. Any object decodes.
public struct IPCAck: Codable, Sendable, Equatable {
    public var result: JSONValue
    public init(result: JSONValue = .object([:])) { self.result = result }
    public init(from decoder: any Decoder) throws {
        result = try decoder.singleValueContainer().decode(JSONValue.self)
    }
    public func encode(to encoder: any Encoder) throws {
        var c = encoder.singleValueContainer()
        try c.encode(result)
    }
}

/// `hello` (role ui): the first frame on the connection.
public struct HelloRequest: UIRequest, Equatable {
    public static let op = "hello"
    public typealias Reply = HelloResult

    public var role: String = "ui"
    public var appBuild: Int
    public var appVersion: String
    public var protocols: [Int]

    public init(appBuild: Int, appVersion: String, protocols: [Int] = IPCProtocol.supportedProtocols) {
        self.appBuild = appBuild
        self.appVersion = appVersion
        self.protocols = protocols
    }

    enum CodingKeys: String, CodingKey {
        case role
        case appBuild = "app_build"
        case appVersion = "app_version"
        case protocols
    }
}

/// `hello` result. `peekd_version` is optional so an older/newer daemon still decodes.
public struct HelloResult: Codable, Sendable, Equatable {
    public var protocolVersion: Int
    public var peekdVersion: String?

    public init(protocolVersion: Int, peekdVersion: String?) {
        self.protocolVersion = protocolVersion
        self.peekdVersion = peekdVersion
    }

    enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol"
        case peekdVersion = "peekd_version"
    }
}

/// How an answer was given. Voice answers travel as `voice.submit`, so `answer.via` is click or keyboard.
public enum AnswerVia: String, Codable, Sendable, CaseIterable {
    case click
    case keyboard
    case voice
}

/// `answer`: the Carbon answered an ask by click or keyboard.
public struct AnswerRequest: UIRequest, Equatable {
    public static let op = "answer"
    public typealias Reply = IPCAck

    public var sendID: String
    public var askID: String
    public var value: AskValue
    public var via: AnswerVia

    public init(sendID: String, askID: String, value: AskValue, via: AnswerVia) {
        self.sendID = sendID
        self.askID = askID
        self.value = value
        self.via = via
    }

    enum CodingKeys: String, CodingKey {
        case sendID = "send_id"
        case askID = "ask_id"
        case value
        case via
    }

    public func encode(to encoder: any Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(sendID, forKey: .sendID)
        try c.encode(askID, forKey: .askID)
        try c.encode(value.jsonValue, forKey: .value)
        try c.encode(via, forKey: .via)
    }
}

/// `voice.submit`: a finished recording (the WAV is the frame's single blob).
/// With `ask_id` null it is a Carbon-initiated message (§1.9.7).
public struct VoiceSubmitRequest: UIRequest, Equatable {
    public static let op = "voice.submit"
    public static let maxBlobBytes = FrameLimits.maxWAVBytes
    public typealias Reply = VoiceSubmitReply

    public var sendID: String?
    public var askID: String?
    public var slot: SlotIndex
    public var durationMs: Int
    /// Additive (not in the §1.6 table): the slot's context, so a test-partition slot is not confused with production.
    public var context: PeekContext?
    /// Additive: `Locale.preferredLanguages` when `stt_language` is `auto` (§8.7 "STT language").
    public var languages: [String]?

    public init(sendID: String?, askID: String?, slot: SlotIndex, durationMs: Int, context: PeekContext? = nil,
                languages: [String]? = nil) {
        self.sendID = sendID
        self.askID = askID
        self.slot = slot
        self.durationMs = durationMs
        self.context = context
        self.languages = languages
    }

    enum CodingKeys: String, CodingKey {
        case sendID = "send_id"
        case askID = "ask_id"
        case slot
        case durationMs = "duration_ms"
        case context
        case languages
    }

    public func encode(to encoder: any Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(sendID, forKey: .sendID)  // explicit null, as in the §1.6 table
        try c.encode(askID, forKey: .askID)
        try c.encode(slot, forKey: .slot)
        try c.encode(durationMs, forKey: .durationMs)
        try c.encodeIfPresent(context, forKey: .context)
        try c.encodeIfPresent(languages, forKey: .languages)
    }
}

/// peekd's reply to `voice.submit`. For a voice message (`ask_id` null) it names the new message, so its
/// `stt.result` is matched by `message_id` rather than by arrival order; nil for a voice answer and from an older
/// peekd whose reply is empty (the UI then falls back to matching messages in order). Unknown fields are ignored.
public struct VoiceSubmitReply: Codable, Sendable, Equatable {
    public var messageID: String?

    public init(messageID: String? = nil) { self.messageID = messageID }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        messageID = try c.decodeIfPresent(String.self, forKey: .messageID)
    }

    enum CodingKeys: String, CodingKey { case messageID = "message_id" }
}

/// `message`: a typed Carbon-initiated message with no pending ask.
public struct MessageRequest: UIRequest, Equatable {
    public static let op = "message"
    public typealias Reply = IPCAck

    public var slot: SlotIndex
    public var text: String
    public var via: AnswerVia = .keyboard
    /// Additive: see ``VoiceSubmitRequest/context``.
    public var context: PeekContext?

    public init(slot: SlotIndex, text: String, context: PeekContext? = nil) {
        self.slot = slot
        self.text = text
        self.context = context
    }

    enum CodingKeys: String, CodingKey { case slot, text, via, context }
}

public enum DismissGesture: String, Codable, Sendable, CaseIterable {
    case downArrow = "down_arrow"
    case downArrowDouble = "down_arrow_double"
    /// One Esc (a show/speak: the visual left, speech plays on; an ask: dismissed with a second Esc).
    case esc
    /// Two Escs within 0.4 s on a show/speak: the visual left and its audio stopped too (peek 0.1.2).
    case escDouble = "esc_double"
}

/// `dismissed`: the Carbon closed a bubble.
public struct DismissedRequest: UIRequest, Equatable {
    public static let op = "dismissed"
    public typealias Reply = IPCAck

    public var sendID: String
    public var gesture: DismissGesture

    public init(sendID: String, gesture: DismissGesture) {
        self.sendID = sendID
        self.gesture = gesture
    }

    enum CodingKeys: String, CodingKey {
        case sendID = "send_id"
        case gesture
    }
}

/// `speech.done`: playback finished or was stopped.
public struct SpeechDoneRequest: UIRequest, Equatable {
    public static let op = "speech.done"
    public typealias Reply = IPCAck

    public var sendID: String
    public var stoppedByUser: Bool
    public var playedMs: Int
    public var totalMs: Int

    public init(sendID: String, stoppedByUser: Bool, playedMs: Int, totalMs: Int) {
        self.sendID = sendID
        self.stoppedByUser = stoppedByUser
        self.playedMs = playedMs
        self.totalMs = totalMs
    }

    enum CodingKeys: String, CodingKey {
        case sendID = "send_id"
        case stoppedByUser = "stopped_by_user"
        case playedMs = "played_ms"
        case totalMs = "total_ms"
    }
}

public enum ShownDoneReason: String, Codable, Sendable, CaseIterable {
    case auto
    case speechDone = "speech_done"
}

/// `shown.done`: a show slid back on its own.
public struct ShownDoneRequest: UIRequest, Equatable {
    public static let op = "shown.done"
    public typealias Reply = IPCAck

    public var sendID: String
    public var visibleMs: Int
    public var reason: ShownDoneReason

    public init(sendID: String, visibleMs: Int, reason: ShownDoneReason) {
        self.sendID = sendID
        self.visibleMs = visibleMs
        self.reason = reason
    }

    enum CodingKeys: String, CodingKey {
        case sendID = "send_id"
        case visibleMs = "visible_ms"
        case reason
    }
}

/// `focus`: the Carbon summoned a slot (pre-warms the Peek session).
public struct FocusRequest: UIRequest, Equatable {
    public static let op = "focus"
    public typealias Reply = IPCAck

    public var slot: SlotIndex
    /// Additive: see ``VoiceSubmitRequest/context``.
    public var context: PeekContext?

    public init(slot: SlotIndex, context: PeekContext? = nil) {
        self.slot = slot
        self.context = context
    }

    enum CodingKeys: String, CodingKey { case slot, context }
}

/// Why the Carbon can or cannot see bubbles right now (`presence.reason`).
public enum PresenceReason: String, Codable, Sendable, CaseIterable {
    case ok
    /// The screen is locked, or this login session is not on the console (fast user switching).
    case locked
    /// The displays are asleep.
    case asleep
    /// No display is attached (closed lid without an external display).
    case displayOff = "display_off"
}

/// `presence` (additive): whether bubbles can reach the Carbon's eyes. While `available` is false peekd pushes
/// no `peek.show` or TTS: new sends stay queued (`carbon_away`) and are pushed in order once the UI reports
/// `available` again. Sent right after every hello and on every change. A peekd that predates it answers
/// `unknown_op`, which the UI tolerates; a UI that never sends it counts as available.
///
/// ```json
/// {"op":"presence","available":false,"reason":"locked"}
/// ```
public struct PresenceRequest: UIRequest, Equatable {
    public static let op = "presence"
    public typealias Reply = IPCAck

    public var available: Bool
    public var reason: PresenceReason
    /// The Carbon paused all peeks in Peek.app (additive, peek 0.1.2; always sent). peekd still pushes while paused
    /// (the hotkey must find a pending ask here) but reports sends as held and words `queue_full` accordingly.
    public var paused: Bool

    public init(available: Bool, reason: PresenceReason, paused: Bool = false) {
        self.available = available
        self.reason = reason
        self.paused = paused
    }

    /// The Carbon can see bubbles.
    public static let available = PresenceRequest(available: true, reason: .ok, paused: false)

    /// The same screen state with `paused` replaced.
    public func with(paused: Bool) -> PresenceRequest {
        PresenceRequest(available: available, reason: reason, paused: paused)
    }
}

/// `shown` (peek 0.1.2): Peek.app began presenting this send's bubble (its pre-warm started; it is on screen within
/// 0.6 s). peekd then sets `shown_at`, starts the speech, arms its watchdog and sends `peek.send.shown` when asked to.
/// Sent at most once per send per Peek.app process, never for summons. An older peekd answers `unknown_op`.
///
/// ```json
/// {"op":"shown","send_id":"snd_…"}
/// ```
public struct ShownRequest: UIRequest, Equatable {
    public static let op = "shown"
    public typealias Reply = IPCAck

    public var sendID: String

    public init(sendID: String) {
        self.sendID = sendID
    }

    enum CodingKeys: String, CodingKey {
        case sendID = "send_id"
    }
}

public enum DrawingFailureReason: String, Codable, Sendable, CaseIterable {
    case throwsRepeatedly = "throws"
    case overruns
    case oom
}

/// `drawing.error`: a drawing switched to the fallback visual (visual.md A7, B10).
public struct DrawingErrorRequest: UIRequest, Equatable {
    public static let op = "drawing.error"
    public typealias Reply = IPCAck

    public var context: PeekContext
    public var accountID: String
    public var actorID: String
    public var reason: DrawingFailureReason
    public var message: String
    public var stack: String?

    public init(context: PeekContext, accountID: String, actorID: String, reason: DrawingFailureReason, message: String,
                stack: String?) {
        self.context = context
        self.accountID = accountID
        self.actorID = actorID
        self.reason = reason
        self.message = message
        self.stack = stack
    }

    enum CodingKeys: String, CodingKey {
        case context
        case accountID = "account_id"
        case actorID = "actor_id"
        case reason
        case message
        case stack
    }

    public func encode(to encoder: any Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(context, forKey: .context)
        try c.encode(accountID, forKey: .accountID)
        try c.encode(actorID, forKey: .actorID)
        try c.encode(reason, forKey: .reason)
        try c.encode(message, forKey: .message)
        try c.encode(stack, forKey: .stack)
    }
}

/// One telemetry event, relayed by peekd to the backend gateway (§6.3). Same shape as the CLI's.
public struct TelemetryEvent: Codable, Sendable, Equatable {
    public var id: String
    public var type: String
    public var data: JSONValue
    public var metadata: JSONValue?

    public init(id: String = "evt_" + UUID().uuidString.replacingOccurrences(of: "-", with: "").lowercased(),
                type: String, data: JSONValue, metadata: JSONValue? = nil) {
        self.id = id
        self.type = type
        self.data = data
        self.metadata = metadata
    }
}

/// `telemetry`: a batch of UI events.
public struct TelemetryRequest: UIRequest, Equatable {
    public static let op = "telemetry"
    public typealias Reply = IPCAck

    public var events: [TelemetryEvent]

    public init(events: [TelemetryEvent]) { self.events = events }
}

/// `settings.changed`: one settings key changed in the UI (mirrored to settings.json).
public struct SettingsChangedRequest: UIRequest, Equatable {
    public static let op = "settings.changed"
    public typealias Reply = IPCAck

    public var key: String
    public var value: JSONValue

    public init(key: PeekSettings.Key, value: JSONValue) {
        self.key = key.rawValue
        self.value = value
    }

    public init(rawKey: String, value: JSONValue) {
        self.key = rawKey
        self.value = value
    }
}

/// `ui.status` (additive, not in the §1.6 table): what only Peek.app knows, for peekd's `doctor` op and
/// `peek doctor`'s `mic` and `hotkeys` checks. The UI pushes it after every successful hello and whenever
/// one of the values changes; peekd keeps the latest report and returns it (merged into its own fields)
/// from `doctor`. peekd may also *ask* with a peekd → UI `ui.status` request, answered with the same
/// object. An older peekd answers `unknown_op`, which the UI tolerates.
///
/// ```json
/// {"op":"ui.status","mic":"granted","hotkeys":{"modifier":"ctrl+cmd","registered":["ctrl+cmd+1"],
///  "failed":["ctrl+cmd+3"],"problems":["…already taken by another app…"]},"glass":"live",
///  "services":"enabled","app_build":1000,"app_version":"0.1.0"}
/// ```
public struct UIStatusReport: UIRequest, Equatable {
    public static let op = "ui.status"
    public typealias Reply = IPCAck

    public struct Hotkeys: Codable, Sendable, Equatable {
        /// The modifier in use (`settings.hotkey_modifier`), e.g. `ctrl+cmd`.
        public var modifier: String
        /// Shortcuts registered right now, in `register side` form (`ctrl+cmd+5`), by position.
        public var registered: [String]
        /// Shortcuts macOS refused (taken by another app), same form.
        public var failed: [String]
        /// One sentence per refusal, with the fix.
        public var problems: [String]

        public init(modifier: String, registered: [String], failed: [String], problems: [String]) {
            self.modifier = modifier
            self.registered = registered
            self.failed = failed
            self.problems = problems
        }
    }

    /// `granted`, `denied`, `restricted` or `undetermined` (Peek asks the first time the Carbon records).
    public var mic: String
    public var hotkeys: Hotkeys
    /// `live` or `frosted` (the glass self-check).
    public var glass: String
    /// `enabled`, or `disabled` under `PEEK_NO_SERVICES=1`.
    public var services: String
    public var appBuild: Int
    public var appVersion: String

    public init(mic: String, hotkeys: Hotkeys, glass: String, services: String, appBuild: Int, appVersion: String) {
        self.mic = mic
        self.hotkeys = hotkeys
        self.glass = glass
        self.services = services
        self.appBuild = appBuild
        self.appVersion = appVersion
    }

    enum CodingKeys: String, CodingKey {
        case mic, hotkeys, glass, services
        case appBuild = "app_build"
        case appVersion = "app_version"
    }
}

/// Result of peekd's `app.uninstall` relay (`peek app uninstall`): the app accepted and is uninstalling.
public struct UninstallAccepted: Codable, Sendable, Equatable {
    public var accepted: Bool
    /// The bundle that is about to be moved to the Trash.
    public var bundlePath: String
    /// Whether the login item and agent are unregistered too (false under `PEEK_NO_SERVICES=1`).
    public var unregistersServices: Bool

    public init(accepted: Bool, bundlePath: String, unregistersServices: Bool) {
        self.accepted = accepted
        self.bundlePath = bundlePath
        self.unregistersServices = unregistersServices
    }

    enum CodingKeys: String, CodingKey {
        case accepted
        case bundlePath = "bundle_path"
        case unregistersServices = "unregisters_services"
    }
}

// MARK: - peekd → UI events

/// `slots.state`: the full slot table (every context).
public struct SlotsStateEvent: Codable, Sendable, Equatable {
    public static let name = "slots.state"
    public var slots: [SlotState]
    public init(slots: [SlotState]) { self.slots = slots }
}

/// `tts.begin`: a PCM stream for `send_id` starts.
public struct TTSBegin: Codable, Sendable, Equatable {
    public static let name = "tts.begin"
    public var sendID: String
    public var format: String
    public var sampleRate: Int
    public var channels: Int
    public var estFrames: Int?

    public init(sendID: String, format: String = "s16le", sampleRate: Int = 24_000, channels: Int = 1,
                estFrames: Int? = nil) {
        self.sendID = sendID
        self.format = format
        self.sampleRate = sampleRate
        self.channels = channels
        self.estFrames = estFrames
    }

    enum CodingKeys: String, CodingKey {
        case sendID = "send_id"
        case format
        case sampleRate = "sample_rate"
        case channels
        case estFrames = "est_frames"
    }
}

/// `tts.chunk`: raw s16le PCM (the frame's single blob, ≤ 64 KiB).
public struct TTSChunk: Sendable, Equatable {
    public static let name = "tts.chunk"
    public var sendID: String
    public var seq: Int
    public var pcm: Data

    public init(sendID: String, seq: Int, pcm: Data) {
        self.sendID = sendID
        self.seq = seq
        self.pcm = pcm
    }

    struct Header: Codable {
        var sendID: String
        var seq: Int
        enum CodingKeys: String, CodingKey {
            case sendID = "send_id"
            case seq
        }
    }
}

/// `tts.end`: the stream is complete; `total_frames` makes progress exact.
public struct TTSEnd: Codable, Sendable, Equatable {
    public static let name = "tts.end"
    public var sendID: String
    public var totalFrames: Int

    public init(sendID: String, totalFrames: Int) {
        self.sendID = sendID
        self.totalFrames = totalFrames
    }

    enum CodingKeys: String, CodingKey {
        case sendID = "send_id"
        case totalFrames = "total_frames"
    }
}

/// `tts.error`: synthesis failed (after peekd's retries).
public struct TTSErrorEvent: Codable, Sendable, Equatable {
    public static let name = "tts.error"
    public var sendID: String
    public var error: IPCErrorBody

    public init(sendID: String, error: IPCErrorBody) {
        self.sendID = sendID
        self.error = error
    }

    enum CodingKeys: String, CodingKey {
        case sendID = "send_id"
        case error
    }
}

/// Why peekd withdrew a bubble. Unknown future reasons decode as ``other(_:)``.
public enum PeekCancelReason: Sendable, Hashable, Codable, CustomStringConvertible {
    case cancelledBySilicon
    case unregistered
    case expired
    /// The Silicon's own `peek send --replace` took the bubble over; its `peek.show` follows (peek 0.1.2).
    case replaced
    case other(String)

    public init(rawValue: String) {
        switch rawValue {
        case "cancelled_by_silicon": self = .cancelledBySilicon
        case "unregistered": self = .unregistered
        case "expired": self = .expired
        case "replaced": self = .replaced
        default: self = .other(rawValue)
        }
    }

    public var rawValue: String {
        switch self {
        case .cancelledBySilicon: "cancelled_by_silicon"
        case .unregistered: "unregistered"
        case .expired: "expired"
        case .replaced: "replaced"
        case .other(let value): value
        }
    }

    public var description: String { rawValue }
    public init(from decoder: any Decoder) throws { self.init(rawValue: try decoder.singleValueContainer().decode(String.self)) }
    public func encode(to encoder: any Encoder) throws {
        var c = encoder.singleValueContainer()
        try c.encode(rawValue)
    }
}

/// `peek.cancel`: slide the bubble for `send_id` out; nothing is sent.
public struct PeekCancelEvent: Codable, Sendable, Equatable {
    public static let name = "peek.cancel"
    public var sendID: String
    public var reason: PeekCancelReason

    public init(sendID: String, reason: PeekCancelReason) {
        self.sendID = sendID
        self.reason = reason
    }

    enum CodingKeys: String, CodingKey {
        case sendID = "send_id"
        case reason
    }
}

/// Outcome of transcribing a voice answer (§1.9.5).
public enum STTOutcome: Sendable, Hashable, Codable, CustomStringConvertible {
    case matched
    case unmatched
    case empty
    case failed
    case other(String)

    public init(rawValue: String) {
        switch rawValue {
        case "matched": self = .matched
        case "unmatched": self = .unmatched
        case "empty": self = .empty
        case "failed": self = .failed
        default: self = .other(rawValue)
        }
    }

    public var rawValue: String {
        switch self {
        case .matched: "matched"
        case .unmatched: "unmatched"
        case .empty: "empty"
        case .failed: "failed"
        case .other(let value): value
        }
    }

    public var description: String { rawValue }
    public init(from decoder: any Decoder) throws { self.init(rawValue: try decoder.singleValueContainer().decode(String.self)) }
    public func encode(to encoder: any Encoder) throws {
        var c = encoder.singleValueContainer()
        try c.encode(rawValue)
    }
}

/// `stt.result`: the transcript was matched (or not) against an ask, or a message was delivered.
public struct STTResultEvent: Codable, Sendable, Equatable {
    public static let name = "stt.result"
    public var askID: String?
    public var messageID: String?
    public var outcome: STTOutcome
    /// The matched answer value, in the same raw shape as ``AskValue/jsonValue``.
    public var value: JSONValue?
    public var error: IPCErrorBody?

    public init(askID: String?, messageID: String?, outcome: STTOutcome, value: JSONValue? = nil,
                error: IPCErrorBody? = nil) {
        self.askID = askID
        self.messageID = messageID
        self.outcome = outcome
        self.value = value
        self.error = error
    }

    enum CodingKeys: String, CodingKey {
        case askID = "ask_id"
        case messageID = "message_id"
        case outcome
        case value
        case error
    }
}

/// `queue.state` (peek 0.1.2): the number of the Silicon's sends waiting behind its current bubble changed while that
/// bubble is pushed. Updates the "+N" badge next to the down-arrow; 0 hides it. Ignored unless `send_id` is the
/// bubble on screen for that slot and context.
///
/// ```json
/// {"v":1,"event":"queue.state","slot":3,"context":"production","send_id":"snd_…","waiting":2}
/// ```
public struct QueueStateEvent: Codable, Sendable, Equatable {
    public static let name = "queue.state"

    public var slot: SlotIndex
    public var context: PeekContext
    /// The current send the badge belongs to.
    public var sendID: String
    /// Waiting + due-waiting sends of this Silicon.
    public var waiting: Int

    public init(slot: SlotIndex, context: PeekContext = .production, sendID: String, waiting: Int) {
        self.slot = slot
        self.context = context
        self.sendID = sendID
        self.waiting = waiting
    }

    enum CodingKeys: String, CodingKey {
        case slot, context
        case sendID = "send_id"
        case waiting
    }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        slot = try c.decode(SlotIndex.self, forKey: .slot)
        context = try c.decodeIfPresent(PeekContext.self, forKey: .context) ?? .production
        sendID = try c.decode(String.self, forKey: .sendID)
        waiting = max(0, try c.decode(Int.self, forKey: .waiting))
    }
}

/// `restarting`: peekd is about to swap Peek.app for build `to_build`.
public struct RestartingEvent: Codable, Sendable, Equatable {
    public static let name = "restarting"
    public var toBuild: Int

    public init(toBuild: Int) { self.toBuild = toBuild }

    enum CodingKeys: String, CodingKey { case toBuild = "to_build" }
}

/// Every event peekd sends to the UI, decoded.
public enum DaemonEvent: Sendable, Equatable {
    case slotsState(SlotsStateEvent)
    case peekShow(PeekShowEvent)
    case ttsBegin(TTSBegin)
    case ttsChunk(TTSChunk)
    case ttsEnd(TTSEnd)
    case ttsError(TTSErrorEvent)
    case peekCancel(PeekCancelEvent)
    case sttResult(STTResultEvent)
    case restarting(RestartingEvent)
    /// peek 0.1.2: the waiting count behind a Silicon's current bubble changed (the "+N" badge).
    case queueState(QueueStateEvent)
    /// An event this build does not know (additive protocol change). Log and ignore.
    case unknown(name: String, fields: JSONValue)
    /// A known event whose fields do not decode. Log; never crash on it.
    case malformed(name: String, reason: String)

    public var name: String {
        switch self {
        case .slotsState: SlotsStateEvent.name
        case .peekShow: PeekShowEvent.name
        case .ttsBegin: TTSBegin.name
        case .ttsChunk: TTSChunk.name
        case .ttsEnd: TTSEnd.name
        case .ttsError: TTSErrorEvent.name
        case .peekCancel: PeekCancelEvent.name
        case .sttResult: STTResultEvent.name
        case .restarting: RestartingEvent.name
        case .queueState: QueueStateEvent.name
        case .unknown(let name, _), .malformed(let name, _): name
        }
    }

    /// Decodes an event frame. Never throws: problems become ``malformed(name:reason:)``.
    public init(name: String, frame: Frame) {
        do throws(FrameCoding.CodingFailure) {
            let header = frame.header
            switch name {
            case SlotsStateEvent.name: self = .slotsState(try FrameCoding.decode(SlotsStateEvent.self, from: header))
            case PeekShowEvent.name: self = .peekShow(try FrameCoding.decode(PeekShowEvent.self, from: header))
            case TTSBegin.name: self = .ttsBegin(try FrameCoding.decode(TTSBegin.self, from: header))
            case TTSChunk.name:
                let head = try FrameCoding.decode(TTSChunk.Header.self, from: header)
                guard frame.blobs.count == 1 else {
                    self = .malformed(name: name, reason: "tts.chunk must carry exactly one blob, got \(frame.blobs.count)")
                    return
                }
                guard frame.blobs[0].count <= FrameLimits.maxTTSChunkBytes else {
                    self = .malformed(
                        name: name,
                        reason: "tts.chunk blob is \(frame.blobs[0].count) bytes; the limit is \(FrameLimits.maxTTSChunkBytes)")
                    return
                }
                self = .ttsChunk(TTSChunk(sendID: head.sendID, seq: head.seq, pcm: frame.blobs[0]))
            case TTSEnd.name: self = .ttsEnd(try FrameCoding.decode(TTSEnd.self, from: header))
            case TTSErrorEvent.name: self = .ttsError(try FrameCoding.decode(TTSErrorEvent.self, from: header))
            case PeekCancelEvent.name: self = .peekCancel(try FrameCoding.decode(PeekCancelEvent.self, from: header))
            case STTResultEvent.name: self = .sttResult(try FrameCoding.decode(STTResultEvent.self, from: header))
            case RestartingEvent.name: self = .restarting(try FrameCoding.decode(RestartingEvent.self, from: header))
            case QueueStateEvent.name: self = .queueState(try FrameCoding.decode(QueueStateEvent.self, from: header))
            default:
                var fields = frame.fields
                fields.removeValue(forKey: "v")
                fields.removeValue(forKey: "event")
                self = .unknown(name: name, fields: .object(fields))
            }
        } catch {
            self = .malformed(name: name, reason: error.description)
        }
    }

    /// Encodes the event as a frame (used by fake daemons in tests and by Simulation).
    public func frame() throws(FrameCoding.CodingFailure) -> Frame {
        switch self {
        case .slotsState(let e): return try FrameCoding.event(name: name, payload: e)
        case .peekShow(let e): return try FrameCoding.event(name: name, payload: e)
        case .ttsBegin(let e): return try FrameCoding.event(name: name, payload: e)
        case .ttsChunk(let e):
            return try FrameCoding.event(name: name, payload: TTSChunk.Header(sendID: e.sendID, seq: e.seq), blobs: [e.pcm])
        case .ttsEnd(let e): return try FrameCoding.event(name: name, payload: e)
        case .ttsError(let e): return try FrameCoding.event(name: name, payload: e)
        case .peekCancel(let e): return try FrameCoding.event(name: name, payload: e)
        case .sttResult(let e): return try FrameCoding.event(name: name, payload: e)
        case .restarting(let e): return try FrameCoding.event(name: name, payload: e)
        case .queueState(let e): return try FrameCoding.event(name: name, payload: e)
        case .unknown(let name, let fields):
            var all = fields.objectValue ?? [:]
            all["v"] = .int(IPCProtocol.envelopeVersion)
            all["event"] = .string(name)
            return Frame(fields: all)
        case .malformed(let name, let reason):
            throw FrameCoding.CodingFailure("cannot encode malformed event \(name): \(reason)")
        }
    }
}

// MARK: - peekd → UI requests

/// `drawing.validate`: run visual.md A9 on a script file (peekd owns the path).
public struct DrawingValidateRequest: Codable, Sendable, Equatable {
    public static let op = "drawing.validate"
    public var scriptPath: String
    public var preview: Bool
    public var dumpFrame: Int?

    public init(scriptPath: String, preview: Bool = false, dumpFrame: Int? = nil) {
        self.scriptPath = scriptPath
        self.preview = preview
        self.dumpFrame = dumpFrame
    }

    enum CodingKeys: String, CodingKey {
        case scriptPath = "script_path"
        case preview
        case dumpFrame = "dump_frame"
    }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        scriptPath = try c.decode(String.self, forKey: .scriptPath)
        preview = try c.decodeIfPresent(Bool.self, forKey: .preview) ?? false
        dumpFrame = try c.decodeIfPresent(Int.self, forKey: .dumpFrame)
    }
}

/// `drawing.load`: activate a validated drawing for a Silicon.
public struct DrawingLoadRequest: Codable, Sendable, Equatable {
    public static let op = "drawing.load"
    public var context: PeekContext
    public var accountID: String
    public var actorID: String
    public var slot: SlotIndex?
    public var scriptPath: String
    public var sha256: String

    public init(context: PeekContext, accountID: String, actorID: String, slot: SlotIndex?, scriptPath: String,
                sha256: String) {
        self.context = context
        self.accountID = accountID
        self.actorID = actorID
        self.slot = slot
        self.scriptPath = scriptPath
        self.sha256 = sha256
    }

    public var siliconKey: SiliconKey { SiliconKey(context: context, accountID: accountID, actorID: actorID) }

    enum CodingKeys: String, CodingKey {
        case context
        case accountID = "account_id"
        case actorID = "actor_id"
        case slot
        case scriptPath = "script_path"
        case sha256
    }
}

/// `drawing.load` result.
public struct DrawingLoadResult: Codable, Sendable, Equatable {
    public var ok: Bool
    public init(ok: Bool) { self.ok = ok }
}

/// `app.update.prepare` / `app.quit`: peekd wants to swap Peek.app for `build`.
public struct AppBuildRequest: Codable, Sendable, Equatable {
    public var build: Int
    public init(build: Int) { self.build = build }
}

/// Result of `app.update.prepare` / `app.quit`: ready only when no bubble is visible,
/// no recording runs and no ask is on screen.
public struct ReadyResult: Codable, Sendable, Equatable {
    public var ready: Bool
    public init(ready: Bool) { self.ready = ready }
}

/// Every request peekd sends to the UI, decoded.
public enum DaemonRequest: Sendable, Equatable {
    public static let appUpdatePrepareOp = "app.update.prepare"
    public static let appQuitOp = "app.quit"
    /// Additive: `peek app uninstall` relayed by peekd. Reply ``UninstallAccepted``, then uninstall and quit.
    public static let appUninstallOp = "app.uninstall"
    /// Additive: peekd asks for the ``UIStatusReport`` (the same object the UI pushes).
    public static let uiStatusOp = "ui.status"
    /// peekd's name for the same question (`UiDoctor` in silicon-peek-client, relayed from `peek doctor`): answered
    /// with the ``UIStatusReport`` too, whose `mic` and `hotkeys.registered/failed` are what it reads.
    public static let doctorOp = "doctor"

    case drawingValidate(DrawingValidateRequest)
    case drawingLoad(DrawingLoadRequest)
    case appUpdatePrepare(AppBuildRequest)
    case appQuit(AppBuildRequest)
    case appUninstall
    case uiStatus
    /// An op this build does not implement; reply with ``IPCErrorBody/unknownOp(_:)``.
    case unknown(op: String, fields: JSONValue)

    public var op: String {
        switch self {
        case .drawingValidate: DrawingValidateRequest.op
        case .drawingLoad: DrawingLoadRequest.op
        case .appUpdatePrepare: Self.appUpdatePrepareOp
        case .appQuit: Self.appQuitOp
        case .appUninstall: Self.appUninstallOp
        case .uiStatus: Self.uiStatusOp
        case .unknown(let op, _): op
        }
    }

    /// Decodes a request frame, or returns the `invalid_request` error to reply with.
    public static func decode(op: String, frame: Frame) -> Result<DaemonRequest, IPCErrorBody> {
        let header = frame.header
        do throws(FrameCoding.CodingFailure) {
            switch op {
            case DrawingValidateRequest.op:
                return .success(.drawingValidate(try FrameCoding.decode(DrawingValidateRequest.self, from: header)))
            case DrawingLoadRequest.op:
                return .success(.drawingLoad(try FrameCoding.decode(DrawingLoadRequest.self, from: header)))
            case appUpdatePrepareOp:
                return .success(.appUpdatePrepare(try FrameCoding.decode(AppBuildRequest.self, from: header)))
            case appQuitOp:
                return .success(.appQuit(try FrameCoding.decode(AppBuildRequest.self, from: header)))
            case appUninstallOp:
                return .success(.appUninstall)
            case uiStatusOp, doctorOp:
                return .success(.uiStatus)
            default:
                var fields = frame.fields
                for key in ["v", "id", "op"] { fields.removeValue(forKey: key) }
                return .success(.unknown(op: op, fields: .object(fields)))
            }
        } catch {
            return .failure(.invalidRequest(op, error.description))
        }
    }
}

/// The UI's answer to a ``DaemonRequest``.
public enum DaemonReply: Sendable, Equatable {
    case ok(result: JSONValue, blobs: [Data])
    case failure(IPCErrorBody)

    public static func encode<T: Encodable>(_ payload: T, blobs: [Data] = []) -> DaemonReply {
        do {
            return .ok(result: .object(try FrameCoding.fields(of: payload)), blobs: blobs)
        } catch {
            return .failure(IPCErrorBody(code: "internal_error", message: "Peek.app could not encode its reply: \(error)"))
        }
    }

    /// The reply to a validation request: the report plus the preview PNG as `bin:[png]`.
    public static func validation(_ report: ValidationReport) -> DaemonReply {
        encode(report, blobs: report.previewPNG.map { [$0] } ?? [])
    }

    public func frame(id: String) -> Frame {
        switch self {
        case .ok(let result, let blobs): FrameCoding.okReply(id: id, result: result, blobs: blobs)
        case .failure(let error): FrameCoding.errorReply(id: id, error: error)
        }
    }
}
