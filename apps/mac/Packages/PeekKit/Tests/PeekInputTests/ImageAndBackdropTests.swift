import AppKit
import Foundation
import PeekCore
import Testing

@testable import PeekInput

@Suite("ImageCache")
@MainActor
struct ImageCacheTests {
    @Test("decodes to at most 512 px with a palette; unreadable images are left out")
    func decode() async throws {
        let directory = try TemporaryDirectory()
        let cover = directory.url.appendingPathComponent("cover.png")
        try TestImages.writePNG(width: 1024, height: 768, to: cover) { context in
            TestImages.fill(context, SRGBColor(red: 0.8, green: 0.2, blue: 0.2), CGRect(x: 0, y: 0, width: 1024, height: 768))
        }
        let garbage = directory.url.appendingPathComponent("broken.png")
        try Data("not a png".utf8).write(to: garbage)
        let cache = ImageCache(paths: PeekPaths(home: directory.url))
        let prepared = await cache.prepare(sendID: "snd_1", paths: [cover.path, garbage.path,
                                                                     directory.url.appendingPathComponent("missing.png").path])
        #expect(prepared.count == 1)
        let image = try #require(prepared[cover.path])
        #expect(image.handle.width == 512 && image.handle.height == 384)
        #expect(image.colors.dominant == "#cc3333")
        #expect(image.colors.palette.count == 3)
        let decoded = try #require(cache.image(for: image.handle))
        #expect(decoded.width == 512)
        // A handle with the right id but the wrong size is not honoured.
        #expect(cache.image(for: ImageHandle(id: image.handle.id, width: 1, height: 1)) == nil)
    }

    @Test("handles belong to one send: the same send gets the same handle, release invalidates it")
    func handlesPerSend() async throws {
        let directory = try TemporaryDirectory()
        let url = directory.url.appendingPathComponent("a.png")
        try TestImages.writePNG(width: 40, height: 20, to: url) { TestImages.fill($0, .black, CGRect(x: 0, y: 0, width: 40, height: 20)) }
        let cache = ImageCache(paths: PeekPaths(home: directory.url))

        async let first = cache.prepare(sendID: "snd_1", paths: [url.path, url.path])
        async let concurrent = cache.prepare(sendID: "snd_1", paths: [url.path])
        let (a, b) = await (first, concurrent)
        #expect(a[url.path] == b[url.path])
        #expect(cache.liveHandleCount == 1)

        let other = await cache.prepare(sendID: "snd_2", paths: [url.path])
        let handle1 = try #require(a[url.path]?.handle)
        let handle2 = try #require(other[url.path]?.handle)
        #expect(handle1.id != handle2.id)
        #expect(cache.liveHandleCount == 2)

        cache.release(sendID: "snd_1")
        #expect(cache.image(for: handle1) == nil)
        #expect(cache.image(for: handle2) != nil)
        cache.release(sendID: "snd_1")
        cache.release(sendID: "snd_2")
        #expect(cache.liveHandleCount == 0)

        let again = await cache.prepare(sendID: "snd_1", paths: [url.path])
        #expect(again[url.path]?.handle.id != handle1.id)
    }

    @Test("a send released while its images decode gets no handles")
    func releasedDuringDecode() async throws {
        let directory = try TemporaryDirectory()
        let url = directory.url.appendingPathComponent("big.png")
        try TestImages.writePNG(width: 2000, height: 2000, to: url) { TestImages.fill($0, .black, CGRect(x: 0, y: 0, width: 2000, height: 2000)) }
        let cache = ImageCache(paths: PeekPaths(home: directory.url))
        let pending = Task { await cache.prepare(sendID: "snd_1", paths: [url.path]) }
        await Task.yield()
        cache.release(sendID: "snd_1")
        let result = await pending.value
        #expect(result.isEmpty)
        #expect(cache.liveHandleCount == 0)
    }

    @Test("relative paths resolve against the image cache directory")
    func relativePaths() async throws {
        let directory = try TemporaryDirectory()
        let paths = PeekPaths(home: directory.url)
        try FileManager.default.createDirectory(at: paths.imageCacheDirectory, withIntermediateDirectories: true)
        try TestImages.writePNG(width: 8, height: 8, to: paths.imageCacheDirectory.appendingPathComponent("abc.png")) {
            TestImages.fill($0, SRGBColor(red: 0, green: 1, blue: 0), CGRect(x: 0, y: 0, width: 8, height: 8))
        }
        let prepared = await ImageCache(paths: paths).prepare(sendID: "snd_1", paths: ["abc.png"])
        #expect(prepared["abc.png"]?.colors.dominant == "#00ff00")
    }
}

@Suite("Wallpaper geometry and decoding")
struct WallpaperTests {
    let screen = CGSize(width: 1600, height: 1000)

    @Test("fill, fit, stretch and centre place the picture like macOS")
    func layouts() {
        let wide = CGSize(width: 3200, height: 1000)
        #expect(WallpaperLayout.imageFrame(imageSize: wide, screenSize: screen, scaling: .scaleProportionallyUpOrDown,
                                           allowClipping: true) == CGRect(x: -800, y: 0, width: 3200, height: 1000))
        #expect(WallpaperLayout.imageFrame(imageSize: wide, screenSize: screen, scaling: .scaleProportionallyUpOrDown,
                                           allowClipping: false) == CGRect(x: 0, y: 250, width: 1600, height: 500))
        #expect(WallpaperLayout.imageFrame(imageSize: wide, screenSize: screen, scaling: .scaleAxesIndependently,
                                           allowClipping: false) == CGRect(x: 0, y: 0, width: 1600, height: 1000))
        #expect(WallpaperLayout.imageFrame(imageSize: CGSize(width: 400, height: 200), screenSize: screen,
                                           scaling: .scaleNone, allowClipping: false)
            == CGRect(x: 600, y: 400, width: 400, height: 200))
        #expect(WallpaperLayout.imageFrame(imageSize: CGSize(width: 400, height: 200), screenSize: screen,
                                           scaling: .scaleProportionallyDown, allowClipping: false)
            == CGRect(x: 600, y: 400, width: 400, height: 200))
    }

    @Test("the sample under a rect mixes picture and fill colour by area, on the right screen")
    func sampling() throws {
        let info = ScreenInfo(id: 2, frame: CGRect(x: 1600, y: 0, width: 1600, height: 1000), backingScale: 1)
        // Top half white, bottom half black (CoreGraphics draws bottom-up).
        let image = try TestImages.image(width: 16, height: 10) { context in
            TestImages.fill(context, .black, CGRect(x: 0, y: 0, width: 16, height: 5))
            TestImages.fill(context, SRGBColor(red: 1, green: 1, blue: 1), CGRect(x: 0, y: 5, width: 16, height: 5))
        }
        let snapshot = WallpaperSnapshot(bitmap: try #require(RGBABitmap(image: image)),
                                         imageFrame: CGRect(x: 0, y: 0, width: 1600, height: 1000),
                                         fillColor: .black, screen: info)
        // A bubble near the top edge of the second screen (AppKit y up).
        let top = try #require(snapshot.averageColor(under: CGRect(x: 2300, y: 800, width: 100, height: 100)))
        #expect(top.hex == "#ffffff")
        let bottom = try #require(snapshot.averageColor(under: CGRect(x: 2300, y: 100, width: 100, height: 100)))
        #expect(bottom.hex == "#000000")
        let straddle = try #require(snapshot.averageColor(under: CGRect(x: 2300, y: 450, width: 100, height: 100)))
        #expect(hexDistance(straddle.hex, "#808080") <= 1)
        #expect(snapshot.averageColor(under: CGRect(x: 0, y: 0, width: 50, height: 50)) == nil)

        // Fit mode: letterbox bars show the fill colour.
        let letterboxed = WallpaperSnapshot(bitmap: RGBABitmap(width: 4, height: 4, fill: SRGBColor(red: 1, green: 1, blue: 1)),
                                            imageFrame: CGRect(x: 0, y: 250, width: 1600, height: 500),
                                            fillColor: SRGBColor(red: 0, green: 0, blue: 1), screen: info)
        let bar = try #require(letterboxed.averageColor(under: CGRect(x: 2000, y: 900, width: 50, height: 50)))
        #expect(bar.hex == "#0000ff")
        let half = try #require(letterboxed.averageColor(under: CGRect(x: 2000, y: 700, width: 100, height: 100)))
        #expect(half.hex == "#8080ff")
    }

    @Test("a screen is picked by the rect's centre, else by overlap")
    func screenChoice() {
        let left = ScreenInfo(id: 1, frame: CGRect(x: 0, y: 0, width: 1000, height: 800))
        let right = ScreenInfo(id: 2, frame: CGRect(x: 1000, y: 0, width: 1000, height: 800))
        #expect(ScreenInfo.screen(for: CGRect(x: 950, y: 10, width: 200, height: 100), in: [left, right]) == right)
        #expect(ScreenInfo.screen(for: CGRect(x: 800, y: 10, width: 100, height: 100), in: [left, right]) == left)
        #expect(ScreenInfo.screen(for: CGRect(x: 5000, y: 10, width: 100, height: 100), in: [left, right]) == nil)
    }

    @Test("the decoder makes a ≤ 256 px thumbnail and places it in points")
    func decoder() throws {
        let directory = try TemporaryDirectory()
        let url = directory.url.appendingPathComponent("wallpaper.png")
        try TestImages.writePNG(width: 3200, height: 2000, to: url) { context in
            TestImages.fill(context, SRGBColor(red: 0.2, green: 0.4, blue: 0.6), CGRect(x: 0, y: 0, width: 3200, height: 2000))
        }
        let info = ScreenInfo(id: 1, frame: CGRect(x: 0, y: 0, width: 1600, height: 1000), backingScale: 2)
        let snapshot = try #require(WallpaperDecoder.snapshot(for: WallpaperKey(url: url, screen: info)))
        #expect(max(snapshot.bitmap.width, snapshot.bitmap.height) == 256)
        #expect(snapshot.imageFrame == CGRect(x: 0, y: 0, width: 1600, height: 1000))
        #expect(snapshot.averageColor(under: CGRect(x: 10, y: 10, width: 100, height: 100))?.hex == "#336699")
        #expect(WallpaperDecoder.snapshot(for: WallpaperKey(url: directory.url.appendingPathComponent("nope.heic"),
                                                            screen: info)) == nil)
    }

    @Test("AppKit rects convert to display-local Quartz rects for ScreenCaptureKit")
    func quartzConversion() {
        let appKit = CGRect(x: 100, y: 800, width: 50, height: 40)
        #expect(ScreenCoordinates.quartzRect(fromAppKit: appKit, primaryHeight: 1000) == CGRect(x: 100, y: 160, width: 50, height: 40))
        // A second display to the right of the primary, top-aligned, 1200 pt tall.
        let secondary = CGRect(x: 1600, y: 0, width: 1920, height: 1200)
        let bubble = CGRect(x: 1700, y: -100, width: 60, height: 60)  // AppKit: below the primary's bottom edge
        #expect(ScreenCoordinates.displayLocalRect(fromAppKit: bubble, displayBounds: secondary, primaryHeight: 1000)
            == CGRect(x: 100, y: 1040, width: 60, height: 60))
    }
}

@Suite("BackdropSampler")
@MainActor
struct BackdropSamplerTests {
    let key = SiliconKey(context: .production, orgID: "tos", actorID: "si:dj")
    let other = SiliconKey(context: .production, orgID: "tos", actorID: "si:ops")
    let screen = ScreenInfo(id: 1, frame: CGRect(x: 0, y: 0, width: 1600, height: 1000), backingScale: 2)
    let bubble = CGRect(x: 700, y: 800, width: 120, height: 120)

    func make(source: BackdropSourceSetting = .wallpaper)
        -> (BackdropSampler, FakeWallpaper, FakeCapture, FakeAppearance)
    {
        let wallpaper = FakeWallpaper(screens: [screen])
        let capture = FakeCapture()
        let appearance = FakeAppearance()
        // The periodic timers never fire during a test: every sample below is started by the test itself, so no
        // background capture can race an assertion.
        let sampler = BackdropSampler(paths: PeekPaths(home: FileManager.default.temporaryDirectory), source: source,
                                      wallpaper: wallpaper, capture: capture, appearance: appearance,
                                      refreshInterval: .seconds(86_400), sampleInterval: .seconds(86_400))
        return (sampler, wallpaper, capture, appearance)
    }

    @Test("untracked keys fall back to the appearance, and follow it")
    func appearanceFallback() {
        let (sampler, _, _, appearance) = make()
        #expect(sampler.backdrop(for: key) == .fromAppearance(.light))
        appearance.set(.dark)
        #expect(sampler.backdrop(for: key) == .fromAppearance(.dark))
    }

    @Test("wallpaper: decodes once per screen, samples under the bubble, notifies on change only")
    func wallpaper() async {
        let (sampler, wallpaper, capture, _) = make()
        wallpaper.show(SRGBColor(red: 0.1, green: 0.1, blue: 0.15), on: screen, name: "night")
        var changes: [(SiliconKey, Backdrop)] = []
        var observed = 0
        sampler.onChange = { changes.append(($0, $1)) }
        sampler.addObserver { _, _ in observed += 1 }
        sampler.track(key, rectOnScreen: bubble)
        #expect(await waitUntil { changes.count == 1 })
        #expect(changes.last?.1.source == .wallpaper)
        #expect(changes.last?.1.tone == .dark)
        #expect(changes.last?.1.ink == "#ffffff")
        #expect(changes.last?.1.color == "#1a1a26")
        #expect(observed == 1)
        #expect(capture.captures.isEmpty)

        sampler.track(other, rectOnScreen: CGRect(x: 100, y: 100, width: 80, height: 80))
        #expect(await waitUntil { changes.count == 2 })
        // Same desktop picture: the cached snapshot is re-sampled synchronously, so nothing can arrive later.
        sampler.refreshWallpapers()
        #expect(changes.count == 2)  // nothing changed, nothing published
        #expect(wallpaper.decodes.count == 1)

        wallpaper.show(SRGBColor(red: 0.95, green: 0.95, blue: 0.9), on: screen, name: "day")
        wallpaper.onEnvironmentChange?()  // e.g. a space switch
        #expect(await waitUntil { changes.count == 4 })
        #expect(sampler.backdrop(for: key).tone == .light)
        #expect(sampler.backdrop(for: key).ink == "#000000")
        #expect(wallpaper.decodes.count == 2)

        sampler.track(key, rectOnScreen: nil)
        #expect(sampler.trackedKeys == [other])
        #expect(sampler.backdrop(for: key).tone == .light)  // the last estimate is kept for the next slide-in
    }

    @Test("an unreadable wallpaper falls back to the appearance")
    func unreadableWallpaper() async {
        let (sampler, wallpaper, _, appearance) = make()
        appearance.current = .dark
        let key = WallpaperKey(url: URL(fileURLWithPath: "/wallpapers/video.mov"), screen: screen)
        wallpaper.keys[screen.id] = key  // no snapshot: decoding fails
        var changes: [Backdrop] = []
        sampler.onChange = { changes.append($1) }
        sampler.track(self.key, rectOnScreen: bubble)
        #expect(await waitUntil { changes.last?.source == .appearance })
        #expect(sampler.backdrop(for: self.key) == .fromAppearance(.dark))
        appearance.set(.light)
        #expect(sampler.backdrop(for: self.key) == .fromAppearance(.light))
    }

    @Test("screen: captures under the bubble with access, uses the wallpaper without it or on failure")
    func screenSource() async {
        let (sampler, wallpaper, capture, _) = make(source: .screen)
        wallpaper.show(SRGBColor(red: 0, green: 0, blue: 0), on: screen, name: "black")
        capture.color = SRGBColor(red: 1, green: 1, blue: 1)
        sampler.track(key, rectOnScreen: bubble)
        #expect(await waitUntil { sampler.backdrop(for: key).source == .screen && sampler.pendingCaptureCount == 0 })
        #expect(capture.captures == [bubble])
        #expect(sampler.backdrop(for: key).tone == .light)

        capture.error = ScreenCaptureError("the display went to sleep")
        sampler.resampleAll()
        #expect(sampler.pendingCaptureCount == 1)
        #expect(await waitUntil { sampler.backdrop(for: key).source == .wallpaper && sampler.pendingCaptureCount == 0 })
        #expect(capture.captures == [bubble, bubble])
        #expect(sampler.backdrop(for: key).tone == .dark)

        // Without access the wallpaper is sampled synchronously and no capture is even started, so the check needs no
        // waiting: nothing is in flight and no timer runs.
        capture.error = nil
        capture.hasAccess = false
        #expect(sampler.effectiveSource == .wallpaper)
        sampler.resampleAll()
        #expect(sampler.pendingCaptureCount == 0)
        #expect(capture.captures == [bubble, bubble])
        #expect(sampler.backdrop(for: key).source == .wallpaper)

        sampler.source = .wallpaper
        #expect(sampler.source == .wallpaper)
    }

    @Test("the tone keeps its hysteresis per bubble across samples")
    func hysteresisAcrossSamples() async {
        let (sampler, wallpaper, _, _) = make()
        wallpaper.show(SRGBColor(red: 0.8, green: 0.8, blue: 0.8), on: screen, name: "a")  // luminance 0.60
        sampler.track(key, rectOnScreen: bubble)
        #expect(await waitUntil { sampler.backdrop(for: key).color == "#cccccc" })
        #expect(sampler.backdrop(for: key).tone == .light)
        wallpaper.show(SRGBColor(red: 0.71, green: 0.71, blue: 0.71), on: screen, name: "b")  // 0.46: stays light
        sampler.refreshWallpapers()
        #expect(await waitUntil { sampler.backdrop(for: key).color == "#b5b5b5" })
        #expect(sampler.backdrop(for: key).tone == .light)
        wallpaper.show(SRGBColor(red: 0.69, green: 0.69, blue: 0.69), on: screen, name: "c")  // 0.434: flips
        sampler.refreshWallpapers()
        #expect(await waitUntil { sampler.backdrop(for: key).tone == .dark })
    }
}

/// Polls `condition` on the main actor.
@MainActor
func waitUntil(timeout: Duration = .seconds(2), _ condition: @MainActor () -> Bool) async -> Bool {
    let deadline = ContinuousClock.now + timeout
    while !condition() {
        if ContinuousClock.now > deadline { return false }
        try? await Task.sleep(for: .milliseconds(2))
    }
    return true
}
