import Foundation
import OSLog
import PeekCore

/// Plays peekd's streamed PCM and exposes what drawings read as `input.speech`
/// (BLUEPRINT §0.1 item 2, §1.9.3, §8.7; notes/speech §4.2):
///
/// * `tts.begin` resets the stream. Each `tts.chunk` (s16le mono, 24 kHz) is cut into 20–50 ms buffers with a
///   1-byte carry and scheduled on the stream's player node with `.dataPlayedBack`.
/// * Playback starts once about 100 ms is buffered (or the stream ended, or 300 ms passed since the first audio).
/// * `level` is the RMS timeline of the scheduled buffers, looked up at the frame being heard (the player's
///   `sampleTime`, minus the device latency) and smoothed with a 30 ms attack and a 150 ms release.
/// * `progress` is played ÷ total: exact after `tts.end`, `max(scheduled, estimate)` before, monotonic, ≤ 0.99
///   until the final buffer has been heard. `done` comes from that buffer's `.dataPlayedBack` completion.
/// * ``stop(sendID:)`` (double click on the down-arrow) stops at once.
/// * When the output device changes, the engine is rebuilt and the stream is rescheduled from the frame heard.
///
/// There is no word or transcript data (D9). Each send reports exactly one of ``onFinished`` or ``onFailed``.
@MainActor
public final class SpeechPlayer: SpeechPlaying {
    public struct Configuration: Sendable, Equatable {
        /// Audio buffered before playback starts, in seconds.
        public var startBuffered: Double
        /// Playback starts after this long even with less buffered (a slow stream still gets heard).
        public var startTimeout: Duration
        /// Speaking rate used to estimate the length from the text when peekd sent no `est_frames`.
        public var charactersPerSecond: Double
        /// Level smoothing time constants, in seconds.
        public var attack: Double
        public var release: Double
        /// RMS window of the level timeline, in seconds.
        public var levelWindow: Double
        /// Subtract the output device latency from the rendered position, so the level follows what is heard.
        public var compensatesLatency: Bool
        /// Latencies above this (seconds) are treated as bogus.
        public var maxLatency: Double
        /// Finished streams kept for ``playback(for:)`` (their audio is released at once).
        public var retainedFinishedStreams: Int
        /// Safety net: when the player's clock has passed the end of the audio by the device latency plus this many
        /// seconds and the final `.dataPlayedBack` still has not arrived, the stream finishes by the clock.
        public var completionGrace: Double
        /// Safety net: a playing stream whose player has not rendered anything for this long ends early.
        public var stallTimeout: Duration
        /// How often the safety nets look.
        public var watchdogInterval: Duration

        public init(startBuffered: Double = 0.100, startTimeout: Duration = .milliseconds(300),
                    charactersPerSecond: Double = 14, attack: Double = 0.030, release: Double = 0.150,
                    levelWindow: Double = 0.010, compensatesLatency: Bool = true, maxLatency: Double = 0.5,
                    retainedFinishedStreams: Int = 32, completionGrace: Double = 0.5, stallTimeout: Duration = .seconds(2),
                    watchdogInterval: Duration = .milliseconds(100)) {
            self.startBuffered = startBuffered
            self.startTimeout = startTimeout
            self.charactersPerSecond = charactersPerSecond
            self.attack = attack
            self.release = release
            self.levelWindow = levelWindow
            self.compensatesLatency = compensatesLatency
            self.maxLatency = maxLatency
            self.retainedFinishedStreams = retainedFinishedStreams
            self.completionGrace = completionGrace
            self.stallTimeout = stallTimeout
            self.watchdogInterval = watchdogInterval
        }
    }

    public var onFinished: (@MainActor (SpeechFinished) -> Void)?
    public var onFailed: (@MainActor (String, IPCErrorBody) -> Void)?

    public let configuration: Configuration
    private let output: any SpeechOutputEngine
    private let now: @MainActor () -> Double
    private var streams: [String: SpeechStream] = [:]
    private var closedOrder: [String] = []
    private var expectedCharacters: [String: Int] = [:]
    private var expectedOrder: [String] = []
    private let logger = PeekLogger(category: "speech")

    /// - Parameters:
    ///   - output: the audio hardware (default: `AVAudioEngine`).
    ///   - now: a monotonic clock in seconds, used for level smoothing.
    public init(output: any SpeechOutputEngine = AVSpeechOutputEngine(), configuration: Configuration = Configuration(),
                now: @escaping @MainActor () -> Double = { ProcessInfo.processInfo.systemUptime }) {
        self.output = output
        self.configuration = configuration
        self.now = now
        output.onConfigurationChange = { [weak self] in self?.recoverFromConfigurationChange() }
    }

    // MARK: SpeechPlaying

    public func handle(_ event: TTSStreamEvent) {
        switch event {
        case .begin(let begin): self.begin(begin)
        case .chunk(let chunk): append(chunk)
        case .end(let end): self.end(sendID: end.sendID, announcedFrames: end.totalFrames)
        case .failure(let failure): fail(failure)
        }
    }

    public func stop(sendID: String) {
        guard let stream = streams[sendID], stream.isActive else { return }
        let played = playedFrames(of: stream)
        stream.progress.update(played: played, scheduled: stream.scheduledEnd, estimate: stream.estimateFrames,
                               total: stream.totalFrames, done: false)
        close(stream, as: .finished(stoppedByUser: true))
        let total = stream.totalFrames ?? max(stream.scheduledEnd, stream.estimateFrames ?? 0, played)
        onFinished?(SpeechFinished(sendID: sendID, stoppedByUser: true, playedMs: stream.milliseconds(played),
                                   totalMs: stream.milliseconds(total)))
    }

    /// Stops every stream that is still buffering or playing (as if the Carbon had double-clicked each).
    public func stopAll() {
        for sendID in streams.values.filter(\.isActive).map(\.sendID).sorted() { stop(sendID: sendID) }
    }

    public func playback(for sendID: String) -> SpeechPlayback? {
        guard let stream = streams[sendID] else { return nil }
        let time = now()
        let dt = stream.lastLevelTime.map { time - $0 } ?? 0
        stream.lastLevelTime = time
        switch stream.state {
        case .failed:
            return nil
        case .buffering:
            let progress = stream.progress.update(played: 0, scheduled: stream.scheduledEnd,
                                                  estimate: stream.estimateFrames, total: stream.totalFrames,
                                                  done: false)
            return SpeechPlayback(sendID: sendID, level: stream.smoother.step(toward: 0, dt: dt), progress: progress,
                                  done: false, started: false, playedMs: 0,
                                  totalMs: stream.totalFrames.map(stream.milliseconds))
        case .playing:
            let played = playedFrames(of: stream)
            let level = stream.smoother.step(toward: stream.timeline.level(atFrame: played), dt: dt)
            let progress = stream.progress.update(played: played, scheduled: stream.scheduledEnd,
                                                  estimate: stream.estimateFrames, total: stream.totalFrames,
                                                  done: false)
            return SpeechPlayback(sendID: sendID, level: level, progress: progress, done: false, started: true,
                                  playedMs: stream.milliseconds(played),
                                  totalMs: stream.totalFrames.map(stream.milliseconds))
        case .finished:
            return SpeechPlayback(sendID: sendID, level: stream.smoother.step(toward: 0, dt: dt),
                                  progress: stream.progress.value, done: true, started: stream.started,
                                  playedMs: stream.milliseconds(stream.playedFrames),
                                  totalMs: stream.milliseconds(stream.totalFrames ?? stream.playedFrames))
        }
    }

    /// Optional hint: the speak text's length, used for `progress` before `tts.end` when peekd sent no
    /// `est_frames` (§8.7: `chars ÷ 14 × 24000`). Call it when the `peek.show` with `speak` arrives.
    public func expectSpeech(sendID: String, characterCount: Int) {
        guard characterCount > 0 else { return }
        if let stream = streams[sendID] {
            if stream.isActive, stream.estimateFrames == nil {
                stream.estimateFrames = estimateFrames(characters: characterCount, sampleRate: stream.sampleRate)
            }
            return
        }
        if expectedCharacters.updateValue(characterCount, forKey: sendID) == nil { expectedOrder.append(sendID) }
        while expectedOrder.count > 64 { expectedCharacters.removeValue(forKey: expectedOrder.removeFirst()) }
    }

    /// Send ids whose audio is buffering or playing.
    public var activeSendIDs: [String] { streams.values.filter(\.isActive).map(\.sendID).sorted() }

    // MARK: Stream events

    private func begin(_ event: TTSBegin) {
        if let previous = streams.removeValue(forKey: event.sendID) {
            // tts.begin resets the timeline: a retry by peekd replaces whatever was there, silently.
            previous.discardVoice()
        }
        closedOrder.removeAll { $0 == event.sendID }
        let characters = expectedCharacters.removeValue(forKey: event.sendID)
        expectedOrder.removeAll { $0 == event.sendID }

        guard event.format == "s16le", event.channels == 1, (8_000...96_000).contains(event.sampleRate) else {
            let stream = SpeechStream(sendID: event.sendID, sampleRate: 24_000, configuration: configuration)
            streams[event.sendID] = stream
            close(stream, as: .failed)
            onFailed?(event.sendID, IPCErrorBody(
                code: "speech_format_unsupported",
                message: "peekd sent speech as \(event.format), \(event.channels) channel(s) at \(event.sampleRate) Hz; "
                    + "Peek.app plays only s16le mono at 8–96 kHz",
                hint: "update Peek.app and the peek CLI so peekd and the app speak the same protocol",
                retryable: false))
            return
        }
        let stream = SpeechStream(sendID: event.sendID, sampleRate: event.sampleRate, configuration: configuration)
        if let frames = event.estFrames, frames > 0 {
            stream.estimateFrames = frames
        } else if let characters {
            stream.estimateFrames = estimateFrames(characters: characters, sampleRate: event.sampleRate)
        }
        streams[event.sendID] = stream
    }

    private func append(_ chunk: TTSChunk) {
        guard let stream = streams[chunk.sendID] else {
            logger.debug("dropping tts.chunk \(chunk.seq) for \(chunk.sendID): no tts.begin")
            return
        }
        // Chunks after a stop, a failure or the end are dropped (peekd may still be draining the HTTP body).
        guard stream.isActive, !stream.endReceived else { return }
        if chunk.seq != stream.nextSeq {
            let id = chunk.sendID
            logger.notice("tts.chunk for \(id): expected seq \(stream.nextSeq), got \(chunk.seq)")
        }
        stream.nextSeq = chunk.seq + 1
        if stream.firstAudioTime == nil, !chunk.pcm.isEmpty {
            stream.firstAudioTime = now()
            scheduleStartTimeout(for: stream)
        }
        schedule(stream.chunker.append(chunk.pcm), on: stream)
        startIfReady(stream)
    }

    private func end(sendID: String, announcedFrames: Int) {
        guard let stream = streams[sendID], stream.isActive, !stream.endReceived else { return }
        stream.endReceived = true
        schedule(stream.chunker.finish(), on: stream)
        guard stream.isActive else { return }
        let received = stream.chunker.framesEmitted
        if announcedFrames != received {
            let id = sendID
            logger.notice("tts.end for \(id) announced \(announcedFrames) frames, \(received) arrived")
        }
        stream.totalFrames = received
        guard received > 0 else {
            failBeforeAudio(stream, IPCErrorBody(
                code: "speech_empty", message: "the speech stream for \(sendID) ended without any audio",
                hint: "the text is shown instead", retryable: false))
            return
        }
        startIfReady(stream)
        if stream.state == .playing, stream.lastPlayedIndex >= stream.segments.count - 1 {
            // Everything was already heard before tts.end arrived (the stream had run dry).
            finishNaturally(stream)
        }
    }

    private func fail(_ failure: TTSErrorEvent) {
        guard let stream = streams[failure.sendID] else {
            let stream = SpeechStream(sendID: failure.sendID, sampleRate: 24_000, configuration: configuration)
            streams[failure.sendID] = stream
            close(stream, as: .failed)
            onFailed?(failure.sendID, failure.error)
            return
        }
        guard stream.isActive, !stream.endReceived else { return }
        if stream.started {
            // Audio is already audible: play what arrived and finish. Never retry after playback started (§1.9.3).
            let id = failure.sendID
            let code = failure.error.code
            logger.notice("speech for \(id) failed mid-stream (\(code)); playing it out")
            end(sendID: failure.sendID, announcedFrames: stream.chunker.framesDecoded)
        } else {
            failBeforeAudio(stream, failure.error)
        }
    }

    // MARK: Scheduling

    private func schedule(_ segments: [PCMChunker.Segment], on stream: SpeechStream) {
        for segment in segments {
            let index = stream.segments.count
            stream.segments.append(segment)
            stream.scheduledEnd = segment.endFrame
            stream.timeline.append(segment.samples, startFrame: segment.startFrame)
            guard ensureVoice(for: stream) else { return }
            enqueue(segment.samples, startFrame: segment.startFrame, index: index, on: stream)
        }
        playIfNeeded(stream)
    }

    private func enqueue(_ samples: [Float], startFrame: Int, index: Int, on stream: SpeechStream) {
        guard let voice = stream.voice else { return }
        if stream.state == .playing, stream.lastPlayedIndex >= stream.voiceScheduledThrough {
            // The player ran dry and its clock kept going through the silence: re-anchor so the new audio
            // counts from the frame it starts at.
            stream.anchorRenderedFrames = voice.renderedFrames ?? stream.anchorRenderedFrames
            stream.anchorStreamFrame = startFrame
        }
        let sendID = stream.sendID
        let generation = stream.generation
        voice.schedule(samples) { [weak self] in
            self?.segmentPlayedBack(sendID: sendID, generation: generation, index: index)
        }
        stream.voiceScheduledThrough = index
        stream.voiceScheduledEnd = startFrame + samples.count
    }

    private func ensureVoice(for stream: SpeechStream) -> Bool {
        if stream.voice != nil { return true }
        do throws(SpeechOutputError) {
            stream.voice = try output.makeVoice(sampleRate: Double(stream.sampleRate))
            stream.voicePlaying = false
            return true
        } catch {
            logger.error("speech output for \(stream.sendID): \(error.description)")
            if stream.started {
                endEarly(stream)
            } else {
                failBeforeAudio(stream, IPCErrorBody(
                    code: "speech_output_unavailable", message: error.description,
                    hint: "check the output device in System Settings › Sound; the text is shown instead",
                    retryable: true))
            }
            return false
        }
    }

    private func startIfReady(_ stream: SpeechStream) {
        guard stream.state == .buffering, stream.voice != nil, stream.scheduledEnd > 0 else { return }
        let threshold = Int(configuration.startBuffered * Double(stream.sampleRate))
        guard stream.scheduledEnd >= threshold || stream.endReceived || stream.startTimedOut else { return }
        stream.state = .playing
        stream.started = true
        stream.anchorStreamFrame = 0
        stream.anchorRenderedFrames = 0
        playIfNeeded(stream)
        armWatchdog(for: stream)
    }

    private func playIfNeeded(_ stream: SpeechStream) {
        guard stream.state == .playing, !stream.voicePlaying, let voice = stream.voice else { return }
        voice.play()
        stream.voicePlaying = true
    }

    private func scheduleStartTimeout(for stream: SpeechStream) {
        let delay = configuration.startTimeout
        Task { [weak self, weak stream] in
            try? await Task.sleep(for: delay)
            guard let self, let stream, self.streams[stream.sendID] === stream else { return }
            stream.startTimedOut = true
            self.startIfReady(stream)
        }
    }

    private func segmentPlayedBack(sendID: String, generation: Int, index: Int) {
        guard let stream = streams[sendID], stream.generation == generation, stream.state == .playing,
            index < stream.segments.count
        else { return }
        if index > stream.lastPlayedIndex {
            stream.lastPlayedIndex = index
            stream.heardFrames = stream.segments[index].endFrame
            stream.playedFrames = max(stream.playedFrames, stream.heardFrames)
        }
        if stream.endReceived, index >= stream.segments.count - 1 { finishNaturally(stream) }
    }

    /// The frame being heard now: the player's rendered position relative to its anchor, minus the device
    /// latency, never behind what `.dataPlayedBack` already confirmed, never ahead of what was scheduled.
    private func playedFrames(of stream: SpeechStream) -> Int {
        guard stream.started else { return 0 }
        guard stream.isActive else { return stream.playedFrames }
        var played = max(stream.playedFrames, stream.heardFrames)
        if let rendered = stream.voice?.renderedFrames {
            let candidate = stream.anchorStreamFrame + (rendered - stream.anchorRenderedFrames)
                - latencyFrames(sampleRate: stream.sampleRate)
            played = max(played, min(candidate, stream.voiceScheduledEnd))
        }
        played = min(played, stream.scheduledEnd)
        stream.playedFrames = played
        return played
    }

    private func latencyFrames(sampleRate: Int) -> Int {
        guard configuration.compensatesLatency else { return 0 }
        let latency = output.presentationLatency
        guard latency.isFinite, latency > 0 else { return 0 }
        return Int(min(latency, configuration.maxLatency) * Double(sampleRate))
    }

    // MARK: Safety nets

    /// Watches a playing stream for two failure modes that would otherwise leave its bubble waiting for
    /// `speech.done` forever: the final `.dataPlayedBack` never arriving, and the player not rendering at all.
    private func armWatchdog(for stream: SpeechStream) {
        guard stream.watchdog == nil else { return }
        let interval = configuration.watchdogInterval
        stream.watchdog = Task { [weak self, weak stream] in
            while !Task.isCancelled {
                try? await Task.sleep(for: interval)
                guard !Task.isCancelled, let self, let stream, stream.state == .playing,
                    self.streams[stream.sendID] === stream
                else { return }
                self.checkWatchdog(stream)
            }
        }
    }

    private func checkWatchdog(_ stream: SpeechStream) {
        let now = now()
        guard let rendered = stream.voice?.renderedFrames else {
            let since = stream.stalledSince ?? now
            stream.stalledSince = since
            let timeout = configuration.stallTimeout
            let limit = Double(timeout.components.seconds) + Double(timeout.components.attoseconds) / 1e18
            if now - since >= limit {
                let id = stream.sendID
                logger.error("speech for \(id) stopped rendering; ending it early")
                endEarly(stream)
            }
            return
        }
        stream.stalledSince = nil
        guard stream.endReceived, let total = stream.totalFrames else { return }
        let position = stream.anchorStreamFrame + (rendered - stream.anchorRenderedFrames)
        let slack = latencyFrames(sampleRate: stream.sampleRate)
            + Int(max(configuration.completionGrace, 0) * Double(stream.sampleRate))
        guard position >= total + slack else { return }
        let id = stream.sendID
        logger.notice("no .dataPlayedBack for the last buffer of \(id); finishing by the clock")
        finishNaturally(stream)
    }

    // MARK: Device changes

    private func recoverFromConfigurationChange() {
        let active = streams.values.filter { $0.isActive && $0.voice != nil }.sorted { $0.sendID < $1.sendID }
        var resume: [(SpeechStream, Int)] = []
        for stream in active {
            let played = playedFrames(of: stream)
            stream.discardVoice()
            resume.append((stream, played))
        }
        output.reset()
        for (stream, played) in resume { reschedule(stream, from: played) }
    }

    private func reschedule(_ stream: SpeechStream, from played: Int) {
        guard stream.isActive else { return }
        stream.playedFrames = max(stream.playedFrames, played)
        stream.heardFrames = stream.playedFrames
        stream.anchorStreamFrame = stream.playedFrames
        stream.anchorRenderedFrames = 0
        guard let first = stream.segments.firstIndex(where: { $0.endFrame > stream.playedFrames }) else {
            // Everything scheduled so far was heard; new chunks get a fresh voice.
            stream.lastPlayedIndex = stream.segments.count - 1
            stream.voiceScheduledThrough = stream.segments.count - 1
            stream.voiceScheduledEnd = stream.scheduledEnd
            if stream.endReceived, stream.state == .playing { finishNaturally(stream) }
            return
        }
        guard ensureVoice(for: stream) else { return }
        stream.lastPlayedIndex = first - 1
        stream.voiceScheduledThrough = first - 1
        for index in first..<stream.segments.count {
            let segment = stream.segments[index]
            let skip = max(0, stream.playedFrames - segment.startFrame)
            let samples = skip > 0 ? Array(segment.samples[skip...]) : segment.samples
            enqueue(samples, startFrame: segment.startFrame + skip, index: index, on: stream)
        }
        playIfNeeded(stream)
    }

    // MARK: Endings

    private func finishNaturally(_ stream: SpeechStream) {
        let total = stream.totalFrames ?? stream.scheduledEnd
        stream.playedFrames = total
        stream.totalFrames = total
        stream.progress.update(played: total, scheduled: total, estimate: nil, total: total, done: true)
        close(stream, as: .finished(stoppedByUser: false))
        onFinished?(SpeechFinished(sendID: stream.sendID, stoppedByUser: false, playedMs: stream.milliseconds(total),
                                   totalMs: stream.milliseconds(total)))
    }

    /// The output device went away mid-playback and no new one could be opened.
    private func endEarly(_ stream: SpeechStream) {
        let played = stream.playedFrames
        let total = stream.totalFrames ?? max(stream.scheduledEnd, stream.estimateFrames ?? 0)
        close(stream, as: .finished(stoppedByUser: false))
        onFinished?(SpeechFinished(sendID: stream.sendID, stoppedByUser: false, playedMs: stream.milliseconds(played),
                                   totalMs: stream.milliseconds(total)))
    }

    private func failBeforeAudio(_ stream: SpeechStream, _ error: IPCErrorBody) {
        close(stream, as: .failed)
        onFailed?(stream.sendID, error)
    }

    private func close(_ stream: SpeechStream, as state: SpeechStream.State) {
        stream.watchdog?.cancel()
        stream.watchdog = nil
        stream.discardVoice()
        stream.state = state
        stream.releaseAudio()
        closedOrder.removeAll { $0 == stream.sendID }
        closedOrder.append(stream.sendID)
        while closedOrder.count > max(0, configuration.retainedFinishedStreams) {
            let oldest = closedOrder.removeFirst()
            if streams[oldest]?.isActive == false { streams.removeValue(forKey: oldest) }
        }
    }

    private func estimateFrames(characters: Int, sampleRate: Int) -> Int? {
        guard configuration.charactersPerSecond > 0 else { return nil }
        return Int(Double(characters) / configuration.charactersPerSecond * Double(sampleRate))
    }
}

/// Everything known about one send's speech.
@MainActor
final class SpeechStream {
    enum State: Equatable {
        case buffering
        case playing
        case finished(stoppedByUser: Bool)
        case failed
    }

    let sendID: String
    let sampleRate: Int
    var state: State = .buffering
    var chunker: PCMChunker
    var timeline: LevelTimeline
    /// Every buffer so far, kept until the stream closes so a device change can reschedule from any frame.
    var segments: [PCMChunker.Segment] = []
    var scheduledEnd = 0
    var voice: (any SpeechVoice)?
    var voicePlaying = false
    /// Replaced whenever the voice is replaced or dropped; completions of older generations are ignored.
    /// Unique across streams, so a stale callback can never match a new stream that reuses the send id.
    var generation = SpeechStream.nextGeneration()
    var voiceScheduledThrough = -1
    var voiceScheduledEnd = 0
    var estimateFrames: Int?
    var endReceived = false
    var totalFrames: Int?
    var started = false
    var startTimedOut = false
    var firstAudioTime: Double?
    var nextSeq = 0
    var lastPlayedIndex = -1
    var heardFrames = 0
    var anchorStreamFrame = 0
    var anchorRenderedFrames = 0
    var playedFrames = 0
    var progress = SpeechProgress()
    var smoother: LevelSmoother
    var lastLevelTime: Double?
    var watchdog: Task<Void, Never>?
    var stalledSince: Double?

    init(sendID: String, sampleRate: Int, configuration: SpeechPlayer.Configuration) {
        self.sendID = sendID
        self.sampleRate = sampleRate
        chunker = PCMChunker(sampleRate: sampleRate)
        timeline = LevelTimeline(windowFrames: max(1, Int(configuration.levelWindow * Double(sampleRate))))
        smoother = LevelSmoother(attack: configuration.attack, release: configuration.release)
    }

    var isActive: Bool { state == .buffering || state == .playing }

    func milliseconds(_ frames: Int) -> Int {
        Int((Double(max(frames, 0)) * 1000 / Double(sampleRate)).rounded())
    }

    private static var generationCounter = 0

    static func nextGeneration() -> Int {
        generationCounter += 1
        return generationCounter
    }

    func discardVoice() {
        generation = Self.nextGeneration()
        voice?.stop()
        voice?.invalidate()
        voice = nil
        voicePlaying = false
    }

    func releaseAudio() {
        segments = []
        timeline.removeAll()
        chunker = PCMChunker(sampleRate: sampleRate)
    }
}
