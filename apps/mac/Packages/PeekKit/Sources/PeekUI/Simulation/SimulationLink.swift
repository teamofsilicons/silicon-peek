import Foundation
import PeekCore

/// One request a simulated bubble sent where a real one would talk to peekd.
public struct SimulationRequestRecord: Sendable, Equatable {
    public var op: String
    public var fields: JSONValue
    public var blobSizes: [Int]
}

/// The `DaemonLinking` behind Simulation's presenter: an in-process stand-in for peekd.
///
/// It never opens a socket. Requests the bubble sends (`answer`, `dismissed`, `speech.done`,
/// `voice.submit`, …) are validated against the same frame limits as the real link, recorded in
/// ``records`` for the Simulation log, and acknowledged. Events (`tts.*`, `stt.result`) and
/// peekd→UI requests (`drawing.load`) are injected by ``SimulationEngine`` exactly as peekd
/// would send them, so the presenter runs its production code paths. Voice answers are answered
/// with an `stt.result` of `failed`: Simulation never sends audio to OpenAI.
public actor SimulationLink: DaemonLinking {
    public static let peekdVersion = "simulation"
    public static let noTranscriptionCode = "simulation_no_transcription"
    /// How long a simulated "transcription" takes before its `stt.result`.
    public static let transcriptionDelay: Duration = .milliseconds(400)

    /// Every request, in the order the presenter sent it.
    public nonisolated let records: AsyncStream<SimulationRequestRecord>
    private nonisolated let recordContinuation: AsyncStream<SimulationRequestRecord>.Continuation

    private var state: DaemonLinkState = .idle
    private var eventContinuations: [UUID: AsyncStream<DaemonEvent>.Continuation] = [:]
    private var stateContinuations: [UUID: AsyncStream<DaemonLinkState>.Continuation] = [:]
    private var handler: (@Sendable (DaemonRequest) async -> DaemonReply)?

    public init() {
        (records, recordContinuation) = AsyncStream.makeStream(of: SimulationRequestRecord.self)
    }

    // MARK: DaemonLinking

    public func start() {
        guard !state.isConnected else { return }
        setState(.connected(HelloResult(protocolVersion: IPCProtocol.supportedProtocols[0], peekdVersion: Self.peekdVersion)))
    }

    public func stop() {
        setState(.stopped)
        for continuation in eventContinuations.values { continuation.finish() }
        for continuation in stateContinuations.values { continuation.finish() }
        eventContinuations.removeAll()
        stateContinuations.removeAll()
        handler = nil
    }

    public func events() -> AsyncStream<DaemonEvent> {
        let (stream, continuation) = AsyncStream.makeStream(of: DaemonEvent.self, bufferingPolicy: .unbounded)
        if case .stopped = state {
            continuation.finish()
            return stream
        }
        let id = UUID()
        eventContinuations[id] = continuation
        continuation.onTermination = { [weak self] _ in
            Task { await self?.removeEventContinuation(id) }
        }
        return stream
    }

    public func states() -> AsyncStream<DaemonLinkState> {
        let (stream, continuation) = AsyncStream.makeStream(of: DaemonLinkState.self, bufferingPolicy: .bufferingNewest(8))
        continuation.yield(state)
        if case .stopped = state {
            continuation.finish()
            return stream
        }
        let id = UUID()
        stateContinuations[id] = continuation
        continuation.onTermination = { [weak self] _ in
            Task { await self?.removeStateContinuation(id) }
        }
        return stream
    }

    public func send<R: UIRequest>(_ request: R, blobs: [Data], timeout: Duration?) async throws(DaemonLinkError) -> R.Reply {
        guard state.isConnected else {
            throw .notConnected(reason: "the Simulation presenter is not running")
        }
        for blob in blobs where blob.count > R.maxBlobBytes {
            throw .invalidRequest(op: R.op, reason: "a \(blob.count)-byte blob exceeds the \(R.maxBlobBytes)-byte limit for \(R.op)")
        }
        let fields: [String: JSONValue]
        do {
            fields = try FrameCoding.fields(of: request)
        } catch {
            throw .invalidRequest(op: R.op, reason: error.description)
        }
        recordContinuation.yield(SimulationRequestRecord(op: R.op, fields: .object(fields), blobSizes: blobs.map(\.count)))
        var reply: [String: JSONValue] = ["simulated": .bool(true)]
        if R.op == VoiceSubmitRequest.op {
            // Like peekd: a voice message is named in the reply, and its outcome follows the reply.
            let askID = fields["ask_id"]?.stringValue
            let result = Self.noTranscriptionResult(askID: askID)
            if askID == nil, let messageID = result.messageID { reply["message_id"] = .string(messageID) }
            Task { [weak self] in
                try? await Task.sleep(for: Self.transcriptionDelay)
                await self?.push(.sttResult(result))
            }
        }
        do {
            return try FrameCoding.decode(R.Reply.self, from: .object(reply))
        } catch {
            throw .invalidReply(op: R.op, reason: error.description)
        }
    }

    public func setRequestHandler(_ handler: (@Sendable (DaemonRequest) async -> DaemonReply)?) {
        self.handler = handler
    }

    // MARK: Injection (what peekd would send)

    /// Delivers an event to every subscriber, in order.
    public func push(_ event: DaemonEvent) {
        broadcast(event)
    }

    /// Sends a peekd→UI request to the presenter's handler.
    public func request(_ request: DaemonRequest) async -> DaemonReply {
        guard let handler else {
            return .failure(
                IPCErrorBody(
                    code: "unknown_op",
                    message: "the Simulation presenter installed no request handler, so \"\(request.op)\" cannot be delivered"))
        }
        return await handler(request)
    }

    public var hasRequestHandler: Bool { handler != nil }
    public var currentState: DaemonLinkState { state }

    /// The `stt.result` a voice answer gets in Simulation.
    public static func noTranscriptionResult(askID: String?) -> STTResultEvent {
        STTResultEvent(
            askID: askID, messageID: askID == nil ? "cmsg_simulation_\(UUID().uuidString.lowercased())" : nil, outcome: .failed,
            error: IPCErrorBody(
                code: noTranscriptionCode,
                message: "Simulation never sends audio to OpenAI, so voice answers are not transcribed.",
                hint: "Tap an option or type your answer instead.", retryable: false))
    }

    // MARK: Private

    private func setState(_ newState: DaemonLinkState) {
        state = newState
        for continuation in stateContinuations.values { continuation.yield(newState) }
    }

    private func broadcast(_ event: DaemonEvent) {
        for continuation in eventContinuations.values { continuation.yield(event) }
    }

    private func removeEventContinuation(_ id: UUID) { eventContinuations.removeValue(forKey: id) }
    private func removeStateContinuation(_ id: UUID) { stateContinuations.removeValue(forKey: id) }
}
