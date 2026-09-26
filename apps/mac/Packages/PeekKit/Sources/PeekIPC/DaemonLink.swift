import Foundation
import OSLog
import PeekCore

/// Peek.app's connection to peekd (BLUEPRINT §1.6): a POSIX AF_UNIX socket with a
/// peer-uid check, NDJSON + binary frames, `hello` role `ui`, request/reply
/// correlation with timeouts, an ordered event stream, peekd → UI requests, and
/// reconnection with exponential backoff (250 ms → 4 s, ±20% jitter).
public actor DaemonLink: DaemonLinking {
    public struct Backoff: Sendable, Equatable {
        public var initial: Duration
        public var maximum: Duration
        public var multiplier: Double
        /// Fraction of random spread around each delay (0.2 = ±20%).
        public var jitter: Double

        public init(initial: Duration = .milliseconds(250), maximum: Duration = .seconds(4), multiplier: Double = 2,
                    jitter: Double = 0.2) {
            self.initial = initial
            self.maximum = maximum
            self.multiplier = multiplier
            self.jitter = jitter
        }

        /// Delay before attempt `attempt` (1-based) after a failure.
        public func delay(attempt: Int, random: Double = Double.random(in: -1...1)) -> Duration {
            let exponent = Double(max(0, attempt - 1))
            let base = min(initial.seconds * pow(multiplier, exponent), maximum.seconds)
            let spread = base * jitter * min(max(random, -1), 1)
            return .milliseconds(Int(((base + spread) * 1000).rounded()))
        }
    }

    public struct Configuration: Sendable {
        public var socketPath: String
        public var appBuild: Int
        public var appVersion: String
        public var protocols: [Int]
        public var helloTimeout: Duration
        public var requestTimeout: Duration
        public var backoff: Backoff
        /// Check that the socket's directory is ours and mode 0700 (always on in the app).
        public var verifySocketDirectory: Bool

        public init(socketPath: String, appBuild: Int, appVersion: String,
                    protocols: [Int] = IPCProtocol.supportedProtocols, helloTimeout: Duration = .seconds(5),
                    requestTimeout: Duration = IPCProtocol.defaultRequestTimeout, backoff: Backoff = Backoff(),
                    verifySocketDirectory: Bool = true) {
            self.socketPath = socketPath
            self.appBuild = appBuild
            self.appVersion = appVersion
            self.protocols = protocols
            self.helloTimeout = helloTimeout
            self.requestTimeout = requestTimeout
            self.backoff = backoff
            self.verifySocketDirectory = verifySocketDirectory
        }

        /// The app's configuration: socket from ``DaemonSocket/resolvePath(environment:uid:)``,
        /// build and version from the bundle's Info.plist.
        public static func live(bundle: Bundle = .main,
                                environment: [String: String] = ProcessInfo.processInfo.environment) -> Configuration {
            let info = bundle.infoDictionary ?? [:]
            let build = (info["CFBundleVersion"] as? String).flatMap(Int.init) ?? 0
            let version = info["CFBundleShortVersionString"] as? String ?? "0.0.0"
            return Configuration(socketPath: DaemonSocket.resolvePath(environment: environment), appBuild: build,
                                 appVersion: version)
        }
    }

    private struct Pending {
        let op: String
        let continuation: CheckedContinuation<Result<JSONValue, DaemonLinkError>, Never>
        let timeout: Task<Void, Never>
    }

    public nonisolated let configuration: Configuration
    private let logger = PeekLogger(category: "ipc")

    private var state: DaemonLinkState = .idle
    private var running = false
    private var runTask: Task<Void, Never>?
    private var connection: FrameConnection?
    private var pending: [String: Pending] = [:]
    private var eventSubscribers: [UUID: AsyncStream<DaemonEvent>.Continuation] = [:]
    private var stateSubscribers: [UUID: AsyncStream<DaemonLinkState>.Continuation] = [:]
    private var requestHandler: (@Sendable (DaemonRequest) async -> DaemonReply)?
    /// The latest presence the app reported, what this connection's peekd acknowledged, and whether it refused the op.
    private var presence: PresenceRequest?
    private var sentPresence: PresenceRequest?
    private var presenceUnsupported = false
    private var presenceFlushing: FrameConnection?

    public init(configuration: Configuration) {
        self.configuration = configuration
    }

    // MARK: DaemonLinking

    public func start() {
        guard runTask == nil else { return }
        running = true
        runTask = Task { await self.runLoop() }
    }

    public func stop() async {
        guard running || runTask != nil else { return }
        running = false
        connection?.close()
        runTask?.cancel()
        await runTask?.value
        runTask = nil
        failAllPending { .disconnected(op: $0) }
        setState(.stopped)
        for continuation in eventSubscribers.values { continuation.finish() }
        for continuation in stateSubscribers.values { continuation.finish() }
        eventSubscribers.removeAll()
        stateSubscribers.removeAll()
    }

    public func events() -> AsyncStream<DaemonEvent> {
        let (stream, continuation) = AsyncStream.makeStream(of: DaemonEvent.self, bufferingPolicy: .unbounded)
        let id = UUID()
        eventSubscribers[id] = continuation
        continuation.onTermination = { [weak self] _ in
            Task { await self?.removeEventSubscriber(id) }
        }
        return stream
    }

    public func states() -> AsyncStream<DaemonLinkState> {
        let (stream, continuation) = AsyncStream.makeStream(of: DaemonLinkState.self, bufferingPolicy: .unbounded)
        let id = UUID()
        stateSubscribers[id] = continuation
        continuation.yield(state)
        continuation.onTermination = { [weak self] _ in
            Task { await self?.removeStateSubscriber(id) }
        }
        return stream
    }

    /// The current state.
    public var currentState: DaemonLinkState { state }

    public func send<R: UIRequest>(_ request: R, blobs: [Data], timeout: Duration?) async throws(DaemonLinkError)
        -> R.Reply
    {
        guard case .connected = state, let connection else {
            throw .notConnected(reason: notConnectedReason)
        }
        return try await perform(request, blobs: blobs, on: connection, timeout: timeout ?? configuration.requestTimeout)
    }

    public func setRequestHandler(_ handler: (@Sendable (DaemonRequest) async -> DaemonReply)?) {
        requestHandler = handler
    }

    public func setPresence(_ presence: PresenceRequest) async {
        self.presence = presence
        guard case .connected = state, let connection else { return }
        await flushPresence(on: connection)
    }

    /// Sends the newest presence until peekd acknowledged it, one request at a time so a quick lock → unlock
    /// never arrives out of order. An older peekd answers `unknown_op`: then nothing more is sent until the next hello.
    private func flushPresence(on conn: FrameConnection) async {
        guard presenceFlushing !== conn else { return }  // the running loop picks up the newest value
        presenceFlushing = conn
        defer { if presenceFlushing === conn { presenceFlushing = nil } }
        while connection === conn, !presenceUnsupported, let wanted = presence, wanted != sentPresence {
            do throws(DaemonLinkError) {
                _ = try await perform(wanted, blobs: [], on: conn, timeout: configuration.helloTimeout)
                guard connection === conn else { return }
                sentPresence = wanted
                logger.debug("peekd acknowledged presence available=\(wanted.available) reason=\(wanted.reason.rawValue)")
            } catch {
                if case .remote(_, let body) = error, body.code == "unknown_op" {
                    presenceUnsupported = true
                    logger.info("peekd does not take presence (older build); it keeps pushing peeks while the screen is locked")
                } else {
                    logger.notice("presence was not delivered: \(error.description)")
                }
                return
            }
        }
    }

    // MARK: Connection loop

    private func runLoop() async {
        var attempt = 0
        while running, !Task.isCancelled {
            attempt += 1
            setState(.connecting(attempt: attempt))

            let conn: FrameConnection
            do {
                conn = try FrameConnection.open(
                    path: configuration.socketPath, verifyDirectory: configuration.verifySocketDirectory)
            } catch {
                await wait(attempt: attempt, reason: error.description)
                continue
            }
            connection = conn
            let pump = Task { await self.pump(conn) }

            do throws(DaemonLinkError) {
                let hello = try await perform(
                    HelloRequest(appBuild: configuration.appBuild, appVersion: configuration.appVersion,
                                 protocols: configuration.protocols),
                    blobs: [], on: conn, timeout: configuration.helloTimeout)
                guard configuration.protocols.contains(hello.protocolVersion) else {
                    throw .invalidReply(
                        op: HelloRequest.op,
                        reason: "peekd \(hello.peekdVersion ?? "(unknown version)") speaks protocol \(hello.protocolVersion), this Peek.app build speaks \(configuration.protocols); update Peek.app (peek app update)")
                }
                attempt = 0
                logger.info("connected to peekd \(hello.peekdVersion ?? "?") at \(self.configuration.socketPath)")
                // A new (or restarted) peekd knows nothing: presence goes first, before any bubble is pushed to nobody.
                sentPresence = nil
                presenceUnsupported = false
                await flushPresence(on: conn)
                setState(.connected(hello))
            } catch {
                conn.close()
                _ = await pump.value
                connection = nil
                failAllPending { .disconnected(op: $0) }
                await wait(attempt: attempt, reason: "handshake failed: \(error)")
                continue
            }

            let reason = await pump.value
            connection = nil
            failAllPending { .disconnected(op: $0) }
            guard running else { break }
            logger.notice("peekd connection ended: \(reason)")
            attempt = 0
            await wait(attempt: 1, reason: reason)
        }
        if !running { setState(.stopped) }
    }

    private func wait(attempt: Int, reason: String) async {
        guard running else { return }
        let delay = configuration.backoff.delay(attempt: attempt)
        setState(.waiting(retryIn: delay, reason: reason))
        try? await Task.sleep(for: delay)
    }

    /// Consumes one connection's frames until it closes; returns why it closed. Requests still
    /// waiting for a reply on this connection fail at once with `.disconnected`.
    private func pump(_ conn: FrameConnection) async -> String {
        var reason = "peekd connection closed"
        loop: for await input in conn.inputs {
            switch input {
            case .frame(let frame):
                handle(frame, on: conn)
            case .closed(let why):
                reason = why
                break loop
            }
        }
        conn.close()
        failAllPending { .disconnected(op: $0) }
        return reason
    }

    private func handle(_ frame: Frame, on conn: FrameConnection) {
        let envelope: Envelope
        do {
            envelope = try Envelope(frame: frame)
        } catch {
            logger.error("ignoring frame from peekd: \(error.description)")
            return
        }
        switch envelope {
        case .reply(let id, let outcome, _):
            guard let entry = pending.removeValue(forKey: id) else {
                logger.notice("reply for unknown or expired request \(id)")
                return
            }
            entry.timeout.cancel()
            switch outcome {
            case .ok(let result): entry.continuation.resume(returning: .success(result))
            case .failure(let body): entry.continuation.resume(returning: .failure(.remote(op: entry.op, body)))
            }
        case .event(let name, let frame):
            let event = DaemonEvent(name: name, frame: frame)
            if case .malformed(_, let reason) = event {
                logger.error("malformed \(name) event from peekd: \(reason)")
            }
            for continuation in eventSubscribers.values { continuation.yield(event) }
        case .request(let id, let op, let frame):
            let decoded = DaemonRequest.decode(op: op, frame: frame)
            let handler = requestHandler
            Task {
                let reply: DaemonReply
                switch decoded {
                case .failure(let error):
                    reply = .failure(error)
                case .success(.unknown(let op, _)):
                    reply = .failure(.unknownOp(op))
                case .success(let request):
                    if let handler {
                        reply = await handler(request)
                    } else {
                        reply = .failure(.unknownOp(op))
                    }
                }
                await self.reply(reply, to: id, op: op, on: conn)
            }
        }
    }

    private func reply(_ reply: DaemonReply, to id: String, op: String, on conn: FrameConnection) async {
        let data: Data
        do {
            data = try reply.frame(id: id).encoded()
        } catch {
            logger.error("cannot encode the reply to \(op): \(error.description)")
            let fallback = DaemonReply.failure(
                IPCErrorBody(code: "internal_error", message: "Peek.app could not encode its reply to \(op): \(error)"))
            guard let encoded = try? fallback.frame(id: id).encoded() else { return }
            data = encoded
        }
        if case .failure(let error) = await conn.write(data) {
            logger.error("cannot send the reply to \(op): \(error.description)")
        }
    }

    // MARK: Requests

    private func perform<R: UIRequest>(_ request: R, blobs: [Data], on conn: FrameConnection, timeout: Duration)
        async throws(DaemonLinkError) -> R.Reply
    {
        let op = R.op
        for blob in blobs where blob.count > R.maxBlobBytes {
            throw .invalidRequest(op: op, reason: "a blob is \(blob.count) bytes; \(op) allows at most \(R.maxBlobBytes)")
        }
        let id = UUID().uuidString.lowercased()
        let data: Data
        do {
            data = try FrameCoding.request(id: id, op: op, payload: request, blobs: blobs).encoded()
        } catch let error as FrameCoding.CodingFailure {
            throw .invalidRequest(op: op, reason: error.description)
        } catch let error as FrameError {
            throw .invalidRequest(op: op, reason: error.description)
        } catch {
            throw .invalidRequest(op: op, reason: "\(error)")
        }

        let outcome: Result<JSONValue, DaemonLinkError> = await withCheckedContinuation { continuation in
            let timeoutTask = Task { [weak self] in
                try? await Task.sleep(for: timeout)
                guard !Task.isCancelled else { return }
                await self?.expire(id, after: timeout)
            }
            pending[id] = Pending(op: op, continuation: continuation, timeout: timeoutTask)
            Task {
                if case .failure(let error) = await conn.write(data) {
                    self.fail(id, with: .notConnected(reason: error.description))
                }
            }
        }
        switch outcome {
        case .success(let result):
            do {
                return try FrameCoding.decode(R.Reply.self, from: result)
            } catch {
                throw .invalidReply(op: op, reason: error.description)
            }
        case .failure(let error):
            throw error
        }
    }

    private func expire(_ id: String, after timeout: Duration) {
        guard let entry = pending.removeValue(forKey: id) else { return }
        entry.continuation.resume(returning: .failure(.timedOut(op: entry.op, after: timeout)))
    }

    private func fail(_ id: String, with error: DaemonLinkError) {
        guard let entry = pending.removeValue(forKey: id) else { return }
        entry.timeout.cancel()
        entry.continuation.resume(returning: .failure(error))
    }

    private func failAllPending(_ error: (String) -> DaemonLinkError) {
        let entries = pending
        pending.removeAll()
        for entry in entries.values {
            entry.timeout.cancel()
            entry.continuation.resume(returning: .failure(error(entry.op)))
        }
    }

    // MARK: State

    private var notConnectedReason: String {
        switch state {
        case .idle: "DaemonLink.start() has not been called"
        case .connecting(let attempt): "connecting to \(configuration.socketPath) (attempt \(attempt))"
        case .connected: "the connection is closing"
        case .waiting(let retryIn, let reason): "\(reason); retrying in \(retryIn)"
        case .stopped: "DaemonLink was stopped"
        }
    }

    private func setState(_ newState: DaemonLinkState) {
        guard newState != state else { return }
        state = newState
        for continuation in stateSubscribers.values { continuation.yield(newState) }
    }

    private func removeEventSubscriber(_ id: UUID) { eventSubscribers.removeValue(forKey: id) }
    private func removeStateSubscriber(_ id: UUID) { stateSubscribers.removeValue(forKey: id) }
}

extension Duration {
    /// The duration in seconds as a `Double`.
    var seconds: Double {
        let (seconds, attoseconds) = components
        return Double(seconds) + Double(attoseconds) / 1e18
    }
}
