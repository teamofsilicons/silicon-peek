import Foundation
import PeekAudio
import PeekCore
import PeekInput

/// A presenter Simulation can drive: ``PeekPresenting`` plus the lifecycle and settings hooks of
/// ``PeekCoordinator``. Simulation runs its **own** coordinator instance (never the live one) so its
/// requests go to a ``SimulationLink`` instead of peekd and its slot table never mixes with
/// peekd's `slots.state`.
@MainActor
public protocol SimulationPresenter: PeekPresenting {
    func start() async
    func stop() async
    func setSetting(_ key: PeekSettings.Key, _ value: JSONValue)
}

extension PeekCoordinator: SimulationPresenter {}

/// The parts a Simulation presenter is built from.
@MainActor
public struct SimulationPresenterParts {
    public let link: SimulationLink
    public let drawing: any DrawingRuntimeProviding
    public let speech: any SpeechPlaying
    public let mic: any MicRecording
    public let images: any ImageProviding
    public let input: any InputHubbing
    public let backdrop: any BackdropSampling
    public let paths: PeekPaths
    /// The live settings with Simulation's mode applied. Never persisted.
    public let settings: PeekSettings
}

/// Everything Simulation needs from the app, injectable for tests.
@MainActor
public struct SimulationDependencies {
    public var paths: PeekPaths
    /// Where the generated samples go (default `~/Library/Caches/Peek/Simulation/v1`).
    public var assetsDirectory: URL
    /// The live settings to start from (hotkey modifier, display, …).
    public var baseSettings: @MainActor () -> PeekSettings
    /// The live drawing runtime; Simulation wraps it, it never creates a second one.
    public var drawing: any DrawingRuntimeProviding
    public var makeSpeech: @MainActor () -> any SpeechPlaying
    public var makeMic: @MainActor () -> any MicRecording
    public var makeImages: @MainActor () -> any ImageProviding
    public var makeLiveBackdrop: @MainActor () -> any BackdropSampling
    public var makeInputHub:
        @MainActor (any ImageProviding, any SpeechPlaying, any MicRecording, any BackdropSampling) -> any InputHubbing
    public var makePresenter: @MainActor (SimulationPresenterParts) -> any SimulationPresenter
    /// Pause between streamed PCM chunks. peekd forwards Deepgram's body at network speed, faster than real time.
    public var chunkInterval: Duration = .milliseconds(40)

    public init(paths: PeekPaths, assetsDirectory: URL? = nil, baseSettings: @escaping @MainActor () -> PeekSettings,
                drawing: any DrawingRuntimeProviding, makeSpeech: @escaping @MainActor () -> any SpeechPlaying,
                makeMic: @escaping @MainActor () -> any MicRecording,
                makeImages: @escaping @MainActor () -> any ImageProviding,
                makeLiveBackdrop: @escaping @MainActor () -> any BackdropSampling,
                makeInputHub: @escaping @MainActor (
                    any ImageProviding, any SpeechPlaying, any MicRecording, any BackdropSampling
                ) -> any InputHubbing,
                makePresenter: @escaping @MainActor (SimulationPresenterParts) -> any SimulationPresenter) {
        self.paths = paths
        self.assetsDirectory = assetsDirectory ?? SimulationAssets.directory(for: paths)
        self.baseSettings = baseSettings
        self.drawing = drawing
        self.makeSpeech = makeSpeech
        self.makeMic = makeMic
        self.makeImages = makeImages
        self.makeLiveBackdrop = makeLiveBackdrop
        self.makeInputHub = makeInputHub
        self.makePresenter = makePresenter
    }

    /// The production graph: a second ``PeekCoordinator`` with its own speech player, mic, image
    /// cache and input hub, the live drawing runtime, and `persistSettings: false`.
    public static func live(for controls: any PeekControlling) -> SimulationDependencies {
        let paths = controls.paths
        return SimulationDependencies(
            paths: paths,
            baseSettings: { [weak controls] in controls?.settings ?? PeekSettings() },
            drawing: controls.drawing,
            makeSpeech: { SpeechPlayer() },
            makeMic: { MicRecorder() },
            makeImages: { ImageCache(paths: paths) },
            makeLiveBackdrop: { BackdropSampler(paths: paths) },
            makeInputHub: { images, speech, mic, backdrop in
                InputHub(images: images, speech: speech, mic: mic, backdrop: backdrop)
            },
            makePresenter: { parts in
                PeekCoordinator(
                    link: parts.link, drawing: parts.drawing, speech: parts.speech, mic: parts.mic, images: parts.images,
                    input: parts.input, backdrop: parts.backdrop, paths: parts.paths, settings: parts.settings,
                    settingsWarnings: [], persistSettings: false)
            })
    }
}

/// One running Simulation presenter and the objects around it.
@MainActor
final class SimulationPipeline {
    let link: SimulationLink
    let presenter: any SimulationPresenter
    let speech: any SpeechPlaying
    let hub: SimulationInputHub
    let backdrop: SimulatedBackdrop
    let drawing: SimulationDrawingRuntime
    private(set) var mode: DisplayMode

    init(dependencies: SimulationDependencies, mode: DisplayMode) {
        link = SimulationLink()
        drawing = SimulationDrawingRuntime(base: dependencies.drawing)
        speech = dependencies.makeSpeech()
        let mic = dependencies.makeMic()
        let images = dependencies.makeImages()
        backdrop = SimulatedBackdrop(live: dependencies.makeLiveBackdrop())
        hub = SimulationInputHub(base: dependencies.makeInputHub(images, speech, mic, backdrop))
        var settings = dependencies.baseSettings()
        settings.mode = mode
        self.mode = mode
        presenter = dependencies.makePresenter(
            SimulationPresenterParts(
                link: link, drawing: drawing, speech: speech, mic: mic, images: images, input: hub, backdrop: backdrop,
                paths: dependencies.paths, settings: settings))
    }

    func setMode(_ newMode: DisplayMode) {
        guard newMode != mode else { return }
        mode = newMode
        presenter.setSetting(.mode, .string(newMode.rawValue))
    }
}
