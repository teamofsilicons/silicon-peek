import AVFoundation
import AppKit
import Foundation
import PeekCore
import Testing

@testable import PeekAudio
@testable import PeekInput

@Suite("InputHub")
@MainActor
struct InputHubTests {
    let key = SiliconKey(context: .production, orgID: "tos", actorID: "si:dj")
    let frame = CGRect(x: 800, y: 700, width: 100, height: 100)
    let speech = FakeSpeech()
    let mic = FakeMic()
    let backdrop = FakeBackdrop()
    let pointer = FakePointer()
    let appearance = FakeAppearance()
    let images: ImageCache
    let hub: InputHub
    let wakes = WakeLog()

    @MainActor
    final class WakeLog {
        var reasons: [WakeReason] = []
        func take() -> [WakeReason] {
            defer { reasons = [] }
            return reasons
        }
    }

    init() {
        images = ImageCache(paths: PeekPaths(home: FileManager.default.temporaryDirectory))
        hub = InputHub(images: images, speech: speech, mic: mic, backdrop: backdrop, pointer: pointer,
                       appearance: appearance)
        let wakes = self.wakes
        hub.source(for: key).onWake = { wakes.reasons.append($0) }
    }

    func show(_ mutate: (inout BubbleInputState) -> Void = { _ in }) {
        hub.update(key) { state in
            state.slot = .bottomRight
            state.phase = .showing
            state.visualFrameOnScreen = frame
            state.facing = -2.3
            state.sendID = "snd_1"
            mutate(&state)
        }
    }

    @Test("the snapshot carries what PeekUI reported, plus appearance, backdrop and context")
    func snapshotFields() throws {
        show { state in
            state.mode = .compact
            state.context = .testing
            state.glass = .frosted
            state.typingText = "hel"
            state.ask = AskPayload(question: "Keep?", kind: .singleChoice(options: [
                AskOption(id: "a", label: "Keep"), AskOption(id: "b", label: "Skip"),
            ]))
            state.askValue = .choice("a")
            state.askHighlight = "b"
        }
        backdrop.values[key] = Backdrop.sample(red: 0.1, green: 0.1, blue: 0.1, source: .wallpaper, previousTone: nil)
        appearance.current = .dark
        let snapshot = hub.source(for: key).snapshot(t: 2, dt: 0.008)
        #expect(snapshot.t == 2 && snapshot.dt == 0.008)
        #expect(snapshot.slot.index == .bottomRight && snapshot.slot.side == .bottomRight && snapshot.slot.facing == -2.3)
        #expect(snapshot.mode == .compact)
        #expect(snapshot.appearance == .dark)
        #expect(snapshot.backdrop.tone == .dark && snapshot.backdrop.source == .wallpaper)
        #expect(snapshot.phase == .showing)
        #expect(snapshot.context == .testing && snapshot.glass == .frosted)
        #expect(snapshot.typing?.text == "hel")
        #expect(snapshot.ask?.options?.map(\.id) == ["a", "b"])
        #expect(snapshot.ask?.value == .choice("a"))
        #expect(snapshot.ask?.highlight == "b")
        #expect(snapshot.speech == nil)
        #expect(snapshot.mic.level == 0)
        #expect(hub.appearance == .dark)
        #expect(hub.state(for: key)?.slot == .bottomRight)
        // The JSON the drawing gets has no word or transcript fields (D9).
        let json = String(decoding: snapshot.jsonBytes(), as: UTF8.self)
        #expect(!json.contains("word") && !json.contains("transcript"))
    }

    @Test("speech: level, progress and done of the bubble's send while it has speak text")
    func speechInput() throws {
        show { $0.speakText = "Side A" }
        #expect(hub.source(for: key).snapshot(t: 0, dt: 0).speech
            == InputSnapshot.Speech(text: "Side A", level: 0, progress: 0, done: false))
        speech.set("snd_1", level: 0.7, progress: 0.4)
        let playing = try #require(hub.source(for: key).snapshot(t: 0, dt: 0).speech)
        #expect(playing == InputSnapshot.Speech(text: "Side A", level: 0.7, progress: 0.4, done: false))
        speech.set("snd_1", level: 0, progress: 1, done: true)
        #expect(hub.source(for: key).snapshot(t: 0, dt: 0).speech?.done == true)
        speech.set("snd_1", level: 3, progress: 7)  // out-of-range values are clamped
        #expect(hub.source(for: key).snapshot(t: 0, dt: 0).speech?.level == 1)
        #expect(hub.source(for: key).snapshot(t: 0, dt: 0).speech?.progress == 1)
    }

    @Test("the mic level reaches only the bubble that is listening")
    func micInput() {
        mic.level = 0.6
        show()
        #expect(hub.source(for: key).snapshot(t: 0, dt: 0).mic.level == 0)
        hub.update(key) { $0.listening = true }
        #expect(hub.source(for: key).snapshot(t: 0, dt: 0).mic.level == 0.6)
    }

    @Test("hover and mouse come from the pointer in the bubble's units, only while visible")
    func pointerInput() {
        show()
        pointer.location = CGPoint(x: 850, y: 750)  // centre
        var snapshot = hub.source(for: key).snapshot(t: 0, dt: 0)
        #expect(snapshot.hover)
        #expect(snapshot.mouse.x == 50 && snapshot.mouse.y == 50 && snapshot.mouse.inside)
        pointer.location = CGPoint(x: 700, y: 750)
        snapshot = hub.source(for: key).snapshot(t: 0, dt: 0)
        #expect(!snapshot.hover)
        #expect(snapshot.mouse.x == -100 && !snapshot.mouse.inside)
        hub.update(key) { $0.phase = .hidden }
        snapshot = hub.source(for: key).snapshot(t: 0, dt: 0)
        #expect(snapshot.mouse == .outside && !snapshot.hover)
    }

    @Test("updates wake the drawing with the reasons that changed, once each, in a fixed order")
    func wakeReasonsOnUpdate() {
        show()
        #expect(wakes.take() == [.phaseChanged, .send, .slotChanged])
        hub.update(key) { $0.phase = .showing }
        #expect(wakes.take().isEmpty)  // nothing changed
        hub.update(key) {
            $0.phase = .asking
            $0.askValue = .number(3)
            $0.mode = .compact
        }
        #expect(wakes.take() == [.phaseChanged, .answer, .modeChanged])
        hub.update(key) { $0.typingText = "h" }
        #expect(wakes.take() == [.answer])
        hub.update(key) { $0.listening = true }
        #expect(wakes.take() == [.micLevel])
        hub.update(key) { $0.visualFrameOnScreen.origin.x += 5 }
        #expect(wakes.take() == [.slotChanged])
        hub.update(key) { $0.speakText = "hi" }
        #expect(wakes.take() == [.send])
        hub.update(key) { $0.glass = .frosted }
        #expect(wakes.take() == [.modeChanged])
    }

    @Test("appearance changes wake every drawing")
    func appearanceWakes() {
        show()
        _ = wakes.take()
        appearance.set(.dark)
        #expect(wakes.take() == [.appearanceChanged])
    }

    @Test("pointer monitors run only while a bubble is visible; hover edges and moves while hovering wake")
    func pointerWakes() {
        #expect(!pointer.isMonitoring)
        show()
        #expect(pointer.isMonitoring)
        _ = wakes.take()
        pointer.move(to: CGPoint(x: 600, y: 600))
        #expect(wakes.take().isEmpty)  // away from the bubble
        pointer.move(to: CGPoint(x: 850, y: 750))
        #expect(wakes.take() == [.hover])
        pointer.move(to: CGPoint(x: 855, y: 752))
        #expect(wakes.take() == [.pointerMoved])
        pointer.move(to: CGPoint(x: 600, y: 600))
        #expect(wakes.take() == [.hover])
        hub.update(key) { $0.phase = .hidden }
        #expect(!pointer.isMonitoring)
    }

    @Test("audio levels above 0 wake the drawing while speaking or listening; the loop stops afterwards")
    func audioWakes() {
        show { $0.speakText = "hello" }
        #expect(hub.isPollingAudio)
        _ = wakes.take()
        hub.pollAudio()
        #expect(wakes.take().isEmpty)  // not audible yet
        speech.set("snd_1", level: 0.5)
        hub.pollAudio()
        hub.pollAudio()
        #expect(wakes.take() == [.speechLevel, .speechLevel])
        speech.set("snd_1", level: 0, progress: 1, done: true)
        hub.pollAudio()
        #expect(wakes.take() == [.speechLevel])  // back to 0 and done: one last frame
        hub.pollAudio()
        #expect(wakes.take().isEmpty)
        #expect(!hub.isPollingAudio)

        hub.update(key) { $0.listening = true }
        _ = wakes.take()
        #expect(hub.isPollingAudio)
        mic.level = 0.4
        hub.pollAudio()
        #expect(wakes.take() == [.micLevel])
        mic.level = 0
        hub.pollAudio()
        #expect(wakes.take() == [.micLevel])
        hub.update(key) { $0.listening = false }
        _ = wakes.take()
        hub.pollAudio()
        #expect(!hub.isPollingAudio)
    }

    @Test("visible bubbles are tracked by the backdrop sampler; changes are polled for plain samplers")
    func backdropTracking() {
        show()
        #expect(backdrop.tracks.last?.0 == key)
        #expect(backdrop.tracks.last?.1 == frame)
        let count = backdrop.tracks.count
        hub.update(key) { $0.typingText = "x" }
        #expect(backdrop.tracks.count == count)  // same rect: no new track call
        _ = wakes.take()
        backdrop.values[key] = Backdrop.sample(red: 0, green: 0, blue: 0, source: .screen, previousTone: nil)
        hub.pollBackdrops()
        #expect(wakes.take() == [.backdropChanged])
        hub.pollBackdrops()
        #expect(wakes.take().isEmpty)
        hub.update(key) { $0.phase = .hidden }
        #expect(backdrop.tracks.last?.1 == nil)
    }

    @Test("a BackdropSampler notifies the hub directly, next to PeekUI's onChange")
    func samplerBroadcast() async {
        let screen = ScreenInfo(id: 1, frame: CGRect(x: 0, y: 0, width: 1600, height: 1000))
        let wallpaper = FakeWallpaper(screens: [screen])
        wallpaper.show(SRGBColor(red: 0.05, green: 0.05, blue: 0.05), on: screen, name: "dark")
        let sampler = BackdropSampler(paths: PeekPaths(home: FileManager.default.temporaryDirectory),
                                      wallpaper: wallpaper, capture: FakeCapture(), appearance: FakeAppearance())
        var chrome = 0
        sampler.onChange = { _, _ in chrome += 1 }
        let hub = InputHub(images: images, speech: speech, mic: mic, backdrop: sampler, pointer: FakePointer(),
                           appearance: FakeAppearance())
        var reasons: [WakeReason] = []
        hub.source(for: key).onWake = { reasons.append($0) }
        hub.update(key) {
            $0.phase = .showing
            $0.visualFrameOnScreen = frame
        }
        #expect(await waitUntil { reasons.contains(.backdropChanged) })
        #expect(chrome == 1)
        #expect(hub.source(for: key).snapshot(t: 0, dt: 0).backdrop.tone == .dark)
        #expect(sampler.trackedKeys == [key])
        hub.remove(key)
        #expect(sampler.trackedKeys.isEmpty)
    }

    @Test("show and option images become handles; the next send releases the old ones")
    func images() async throws {
        let directory = try TemporaryDirectory()
        let cover = directory.url.appendingPathComponent("cover.png")
        let icon = directory.url.appendingPathComponent("icon.png")
        try TestImages.writePNG(width: 64, height: 64, to: cover) { TestImages.fill($0, SRGBColor(red: 1, green: 0, blue: 0), CGRect(x: 0, y: 0, width: 64, height: 64)) }
        try TestImages.writePNG(width: 32, height: 16, to: icon) { TestImages.fill($0, SRGBColor(red: 0, green: 0, blue: 1), CGRect(x: 0, y: 0, width: 32, height: 16)) }
        show { state in
            state.show = ShowPayload(elements: [.text("Now playing"), .image(path: cover.path, caption: "Side A")])
            state.ask = AskPayload(question: "Next?", kind: .singleChoice(options: [
                AskOption(id: "1", label: "This", image: icon.path), AskOption(id: "2", label: "That"),
            ]))
        }
        // Until decoded, text shows and the image element waits.
        #expect(hub.source(for: key).snapshot(t: 0, dt: 0).show?.elements == [.text("Now playing")])
        #expect(await waitUntil { wakes.reasons.contains(.send) && hub.source(for: key).snapshot(t: 0, dt: 0).show?.elements.count == 2 })
        let snapshot = hub.source(for: key).snapshot(t: 0, dt: 0)
        guard case .image(let handle, let caption, let colors)? = snapshot.show?.elements.last else {
            Issue.record("expected an image element, got \(String(describing: snapshot.show))")
            return
        }
        #expect(caption == "Side A")
        #expect(colors.dominant == "#ff0000")
        #expect(handle.width == 64)
        #expect(images.image(for: handle) != nil)
        let option = try #require(snapshot.ask?.options?.first)
        #expect(option.colors?.dominant == "#0000ff")
        #expect(option.image?.width == 32)
        #expect(snapshot.ask?.options?.last?.image == nil)

        hub.update(key) { state in
            state.sendID = "snd_2"
            state.show = ShowPayload(elements: [.text("Next")])
            state.ask = nil
        }
        #expect(images.image(for: handle) == nil)
        #expect(images.liveHandleCount == 0)
        #expect(hub.source(for: key).snapshot(t: 0, dt: 0).show?.elements == [.text("Next")])
    }

    @Test("remove forgets the bubble, releases its images and stops tracking")
    func remove() {
        show()
        let source = hub.source(for: key)
        hub.remove(key)
        #expect(hub.state(for: key) == nil)
        #expect(backdrop.tracks.last?.1 == nil)
        #expect(!pointer.isMonitoring)
        #expect(hub.source(for: key) === source)  // stable per key
        let fallback = source.snapshot(t: 1, dt: 0)
        #expect(fallback.phase == .hidden && fallback.speech == nil && fallback.show == nil)
    }

    @Test("the real SpeechPlayer and MicRecorder plug into the hub")
    func realAudioTypes() {
        let output = PlaceholderOutput()
        let player = SpeechPlayer(output: output)
        let recorder = MicRecorder(permissions: GrantedMic(), makeDevice: { SilentDevice() })
        let hub = InputHub(images: images, speech: player, mic: recorder, backdrop: backdrop, pointer: FakePointer(),
                           appearance: FakeAppearance())
        hub.update(key) {
            $0.phase = .speaking
            $0.sendID = "snd_9"
            $0.speakText = "Hello"
        }
        player.handle(.begin(TTSBegin(sendID: "snd_9", estFrames: 24_000)))
        let snapshot = hub.source(for: key).snapshot(t: 0, dt: 0)
        #expect(snapshot.speech == InputSnapshot.Speech(text: "Hello", level: 0, progress: 0, done: false))
        #expect(snapshot.mic.level == 0)
    }
}

// Minimal hardware stand-ins for the integration test above.
@MainActor
private final class PlaceholderOutput: SpeechOutputEngine {
    var presentationLatency: TimeInterval = 0
    var onConfigurationChange: (@MainActor () -> Void)?
    func makeVoice(sampleRate: Double) throws(SpeechOutputError) -> any SpeechVoice {
        throw SpeechOutputError("no device in tests")
    }
    func reset() {}
}

@MainActor
private final class GrantedMic: MicPermissionProviding {
    var current: MicPermission { .granted }
    func request() async -> Bool { true }
}

@MainActor
private final class SilentDevice: MicCaptureDevice {
    var onInterrupted: (@MainActor (MicRecordingError) -> Void)?
    func start(onBuffer: @escaping @Sendable (AVAudioPCMBuffer) -> Void) throws(MicRecordingError) {}
    func stop() {}
}
