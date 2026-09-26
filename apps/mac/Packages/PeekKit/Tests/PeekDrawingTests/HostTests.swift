import AppKit
import Foundation
import PeekCore
import Testing

@testable import PeekDrawing

private let key = SiliconKey(context: .production, orgID: "tos", actorID: "si:dj")

@MainActor
private func makeHost(glassMode: GlassMode = .live, images: FakeImages = FakeImages()) -> (DrawingHost, NSView) {
    let host = DrawingHost(key: key, initial: "dj", images: images, glassMode: glassMode, manualClock: true)
    let visual = NSView(frame: NSRect(x: 0, y: 0, width: 120, height: 120))
    return (host, visual)
}

@MainActor
private func load(_ host: DrawingHost, _ source: String, filename: String = "drawing.js") async throws {
    try await host.load(DrawingScript(key: key, sha256: "abc", source: Data(source.utf8), filename: filename))
}

/// Runs one display tick and waits for its frame to come back.
@MainActor
@discardableResult
private func step(_ host: DrawingHost, at time: CFTimeInterval) async -> Bool {
    host.tick(timestamp: time)
    return await waitUntil { !host.isFrameInFlight }
}

@MainActor
private func kinds(_ compositor: Compositor) -> [String] {
    compositor.slots.map { slot in
        switch slot {
        case .draw: "draw"
        case .vibrant: "vibrant"
        case .glass: "glass"
        case .blur: "blur"
        }
    }
}

@Suite("drawing host")
@MainActor
struct HostTests {
    @Test("a loaded drawing renders into glass and draw layer views and hit-tests them (B9)")
    func loadRenderHitTest() async throws {
        let (host, visual) = makeHost()
        try await load(host, SampleDrawings.cassette.source, filename: "cassette.js")
        #expect(host.status == .ready(sha256: "abc"))
        #expect(!host.isAwake)  // loaded, then paused (visual.md A1)

        host.attach(to: visual)
        #expect(host.canvas.superview === visual)
        #expect(host.isAwake)  // never rendered since load: attaching wakes it
        #expect(await step(host, at: 10))
        #expect(host.framesRendered == 1)
        #expect(kinds(host.compositor) == ["glass", "draw"])
        #expect(host.canvas.subviews.count == 2)
        guard case .glass(let glassView) = host.compositor.slots[0] else {
            Issue.record("expected a glass view first")
            return
        }
        #expect(glassView.members.count == 1)
        #expect(glassView.members[0].rule == .evenOdd)
        #expect(!glassView.hasPendingShape)

        #expect(host.isOverContent(unitPoint: CGPoint(x: 50, y: 70)))  // glass body
        #expect(host.isOverContent(unitPoint: CGPoint(x: 50, y: 30)))  // the drawn label
        #expect(!host.isOverContent(unitPoint: CGPoint(x: 30, y: 50)))  // inside a reel hole
        #expect(!host.isOverContent(unitPoint: CGPoint(x: 2, y: 2)))

        // Nothing moves while 'showing': the drawing went back to sleep and ticks do nothing.
        #expect(!host.isAwake)
        host.tick(timestamp: 10.1)
        #expect(!host.isFrameInFlight)
        #expect(host.framesRendered == 1)
    }

    @Test("dt is 0 after waking, the time since the last frame otherwise, capped at 0.1 s (B4)")
    func deltaTime() async throws {
        let (host, visual) = makeHost()
        let input = FakeInputSource()
        host.input = input
        try await load(host, "peek.frame(ctx => { ctx.fillRect(0, 0, 10, 10); return true })")
        host.attach(to: visual)
        await step(host, at: 100)
        await step(host, at: 100.016)
        await step(host, at: 100.5)
        let dts = input.requests.map(\.dt)
        #expect(dts.count == 3)
        #expect(dts[0] == 0)
        #expect(abs(dts[1] - 0.016) < 1e-9)
        #expect(dts[2] == 0.1)
        #expect(input.requests.allSatisfy { $0.t >= 0 })
    }

    @Test("a sleeping drawing wakes on the input's wake signal, and levels above 0 keep it awake")
    func wakeAndLevels() async throws {
        let (host, visual) = makeHost()
        let input = FakeInputSource()
        host.input = input
        try await load(host, "peek.frame(ctx => { ctx.fillRect(0, 0, 10, 10); return false })")
        host.attach(to: visual)
        await step(host, at: 1)
        #expect(!host.isAwake)

        input.onWake?(.hover)
        #expect(host.isAwake)
        input.base.mic = .init(level: 0.4)
        await step(host, at: 1.1)
        #expect(host.isAwake)  // returned false, but the mic is live
        #expect(input.requests.last?.dt == 0)  // first frame after waking
        input.base.mic = .init(level: 0)
        await step(host, at: 1.2)
        #expect(!host.isAwake)
    }

    @Test("events reach the drawing's handlers and wake it")
    func events() async throws {
        let (host, visual) = makeHost()
        var logs: [String] = []
        host.onLog = { logs.append($0) }
        try await load(host, """
            let color = 'blue'
            peek.on('click', e => { color = 'red'; peek.log('click at ' + e.x + ',' + e.y) })
            peek.frame(ctx => { ctx.fillStyle = color; ctx.fillRect(0, 0, 100, 100); return false })
            """)
        host.attach(to: visual)
        await step(host, at: 1)
        guard case .draw(let view) = host.compositor.slots.first else {
            Issue.record("expected a draw view")
            return
        }
        let before = view.contentHash
        host.deliver(.click(x: 12, y: 34, count: 1))
        #expect(host.isAwake)
        await step(host, at: 2)
        #expect(view.contentHash != before)
        #expect(await waitUntil { logs.contains("click at 12,34") })
    }

    @Test("10 throwing frames in a row switch to the fallback visual and report once (B10)")
    func throwsFallback() async throws {
        let (host, visual) = makeHost()
        var failures: [DrawingFailure] = []
        var logs: [String] = []
        host.onFailure = { failures.append($0) }
        host.onLog = { logs.append($0) }
        try await load(host, "peek.frame(ctx => { ctx.fillRect(0, 0, 1, 1); throw new Error('nope') })", filename: "bad.js")
        host.attach(to: visual)
        for index in 0..<12 {
            await step(host, at: Double(index) / 60)
        }
        #expect(failures.count == 1)
        let failure = try #require(failures.first)
        #expect(failure.reason == .throwsRepeatedly)
        #expect(failure.message == "frame() threw 10 times in a row; last error: Error: nope")
        #expect(failure.stack?.contains("bad.js") == true)
        #expect(host.status == .fallback(failure))
        #expect(host.compositor.isShowingFallback)
        #expect(host.canvas.subviews.count == 1)
        #expect(host.isOverContent(unitPoint: CGPoint(x: 50, y: 50)))
        #expect(!host.isOverContent(unitPoint: CGPoint(x: 3, y: 3)))
        #expect(logs.filter { $0.hasPrefix("frame() threw: Error: nope") }.count == 10)
    }

    @Test("a successful frame resets the throw streak")
    func throwStreakResets() async throws {
        let (host, visual) = makeHost()
        try await load(host, """
            let n = 0
            peek.frame(ctx => { ctx.fillRect(0, 0, 1, 1); if (++n % 5 !== 0) throw new Error('flaky'); return true })
            """)
        host.attach(to: visual)
        for index in 0..<25 {
            await step(host, at: Double(index) / 60)
        }
        #expect(host.status == .ready(sha256: "abc"))
    }

    @Test("running out of memory destroys the VM and shows the fallback")
    func outOfMemory() async throws {
        let (host, visual) = makeHost()
        var failures: [DrawingFailure] = []
        host.onFailure = { failures.append($0) }
        try await load(host, """
            const keep = []
            peek.frame(() => { for (;;) keep.push(new Array(100000).fill(1)) })
            """)
        host.attach(to: visual)
        host.tick(timestamp: 1)
        #expect(await waitUntil { !failures.isEmpty })
        #expect(failures.first?.reason == .oom)
        if case .fallback(let failure) = host.status {
            #expect(failure.reason == .oom)
        } else {
            Issue.record("expected the fallback status, got \(host.status)")
        }
    }

    @Test("30 overruns within 5 s switch to the fallback visual")
    func overruns() async throws {
        let (host, visual) = makeHost()
        var failures: [DrawingFailure] = []
        host.onFailure = { failures.append($0) }
        try await load(host, "peek.frame(() => { let s = 0; for (let i = 0; i < 1e9; i++) s += i; return true })")
        host.attach(to: visual)
        for index in 0..<30 where failures.isEmpty {
            await step(host, at: 5 + Double(index) / 60)
        }
        #expect(failures.count == 1)
        #expect(failures.first?.reason == .overruns)
        #expect(failures.first?.message.contains("30 times within 5 s") == true)
    }

    @Test("overruns spread over more than 5 s do not trip the fallback")
    func overrunWindow() async throws {
        let (host, visual) = makeHost()
        try await load(host, "peek.frame(() => { let s = 0; for (let i = 0; i < 1e9; i++) s += i; return true })")
        host.attach(to: visual)
        for index in 0..<31 {
            await step(host, at: Double(index) * 0.2)
        }
        #expect(host.status == .ready(sha256: "abc"))
    }

    @Test("a script that fails to load throws, shows the fallback and does not report through onFailure")
    func loadFailure() async throws {
        let (host, visual) = makeHost()
        var failures: [DrawingFailure] = []
        host.onFailure = { failures.append($0) }
        host.attach(to: visual)
        await #expect(throws: DrawingFailure.self) {
            try await load(host, "peek.frame((ctx) => {", filename: "broken.js")
        }
        guard case .fallback(let failure) = host.status else {
            Issue.record("expected the fallback status")
            return
        }
        #expect(failure.message.hasPrefix("loading broken.js threw: SyntaxError"), "\(failure.message)")
        #expect(failures.isEmpty)
        #expect(host.compositor.isShowingFallback)

        let big = String(repeating: " ", count: DrawingLimits.maxScriptBytes + 1)
        await #expect(throws: DrawingFailure.self) { try await load(host, big, filename: "big.js") }
    }

    @Test("loading again replaces the script and all its state; unload clears everything")
    func reloadAndUnload() async throws {
        let (host, visual) = makeHost()
        var logs: [String] = []
        host.onLog = { logs.append($0) }
        try await load(host, "globalThis.counter = (globalThis.counter ?? 0) + 1; peek.log('counter ' + counter); peek.frame(ctx => { ctx.fillRect(0, 0, 5, 5); return false })")
        try await load(host, "globalThis.counter = (globalThis.counter ?? 0) + 1; peek.log('counter ' + counter); peek.frame(ctx => { ctx.fillRect(0, 0, 5, 5); return false })")
        #expect(await waitUntil { logs.count == 2 })
        #expect(logs == ["counter 1", "counter 1"])
        host.attach(to: visual)
        await step(host, at: 1)
        #expect(host.canvas.subviews.count == 1)
        host.unload()
        #expect(host.status == .empty)
        #expect(host.canvas.subviews.isEmpty)
        host.tick(timestamp: 2)
        #expect(!host.isFrameInFlight)
    }

    @Test("loading into a host that is already on screen renders its first frame right away")
    func loadWhileAttached() async throws {
        let (host, visual) = makeHost()
        host.attach(to: visual)
        try await load(host, "peek.frame(ctx => { ctx.fillRect(0, 0, 50, 50); return false })")
        #expect(host.isAwake)
        await step(host, at: 1)
        #expect(host.framesRendered == 1)
        #expect(host.isOverContent(unitPoint: CGPoint(x: 25, y: 25)))
        #expect(!host.isOverContent(unitPoint: CGPoint(x: 75, y: 75)))
    }

    @Test("glass outline changes apply at most every 100 ms, last write wins (D14)")
    func glassRateLimit() async throws {
        let (host, visual) = makeHost()
        var now: CFTimeInterval = 50
        host.clock = { now }
        var flushes: [(TimeInterval, @MainActor () -> Void)] = []
        host.compositor.scheduleFlush = { delay, work in flushes.append((delay, work)) }
        try await load(host, """
            peek.frame((ctx, input) => { ctx.beginPath(); ctx.arc(50, 50, 20 + input.t * 10, 0, 7); ctx.fillGlass(); return true })
            """)
        host.attach(to: visual)
        for index in 0..<6 {
            now = 50 + Double(index) / 120
            await step(host, at: now)
        }
        #expect(host.compositor.shapeRebuilds == 1)
        #expect(host.compositor.hasPendingShapes)
        #expect(flushes.count == 1)
        let (delay, flush) = try #require(flushes.first)
        #expect(delay > 0 && delay <= DrawingLimits.glassUpdateInterval)
        now = 50.1
        flush()
        #expect(host.compositor.shapeRebuilds == 2)
        #expect(!host.compositor.hasPendingShapes)
        // The applied shape is the latest one (last write wins).
        guard case .glass(let view) = host.compositor.slots[0] else {
            Issue.record("expected a glass view")
            return
        }
        #expect(view.shapeKey == GlassLayerView.shapes(for: view.members).key)
    }

    @Test("transform-only glass changes apply every frame without rebuilding the glass")
    func glassTransformOnly() async throws {
        let (host, visual) = makeHost()
        host.clock = { 0 }  // input.t = timestamp − load time
        try await load(host, """
            const bar = new Path2D('M-20 -5 H20 V5 H-20 Z')
            peek.frame((ctx, input) => { ctx.translate(50, 50); ctx.rotate(input.t); ctx.fillGlass(bar, { style: 'clear' }); return true })
            """)
        host.attach(to: visual)
        var transforms: [CGAffineTransform] = []
        for index in 0..<4 {
            await step(host, at: 20 + Double(index) / 60)
            guard case .glass(let view) = host.compositor.slots[0] else {
                Issue.record("expected a glass view")
                return
            }
            transforms.append(view.unitTransform)
            #expect(!view.hasPendingShape)
        }
        #expect(host.compositor.shapeRebuilds == 1)
        #expect(Set(transforms.map { $0.b }).count == 4)  // rotated a little more every frame
        #expect(transforms[0].tx == 50 && transforms[0].ty == 50)
    }

    @Test("layer views are reused by position when the structure changes")
    func structureChange() async throws {
        let (host, visual) = makeHost()
        try await load(host, """
            let n = 0
            peek.frame(ctx => {
              ctx.fillRect(0, 0, 10, 10)
              if (n++ > 0) { ctx.beginPath(); ctx.rect(20, 20, 40, 40); ctx.fillGlass(); ctx.vibrant = true; ctx.fillRect(0, 0, 5, 5) }
              return true
            })
            """)
        host.attach(to: visual)
        await step(host, at: 1)
        #expect(kinds(host.compositor) == ["draw"])
        let firstView = host.compositor.slots[0].view
        await step(host, at: 1.02)
        #expect(kinds(host.compositor) == ["draw", "glass", "vibrant"])
        #expect(host.compositor.slots[0].view === firstView)
        #expect(host.canvas.subviews.count == 3)
        #expect(host.canvas.subviews.map { ObjectIdentifier($0) } == host.compositor.slots.map { ObjectIdentifier($0.view) })
    }

    @Test("blur layers use NSVisualEffectView with the requested material")
    func blurLayer() async throws {
        let (host, visual) = makeHost()
        try await load(host, "peek.frame(ctx => { ctx.beginPath(); ctx.arc(50, 50, 30, 0, 7); ctx.fillBlur({ material: 'menu' }); return false })")
        host.attach(to: visual)
        await step(host, at: 1)
        guard case .blur(let view) = host.compositor.slots.first else {
            Issue.record("expected a blur view")
            return
        }
        #expect(view.material == .menu)
        #expect(view.maskImage != nil)
        #expect(host.isOverContent(unitPoint: CGPoint(x: 50, y: 50)))
        #expect(!host.isOverContent(unitPoint: CGPoint(x: 5, y: 5)))
    }

    @Test("detach removes the canvas; attach puts the same layers back")
    func detachAttach() async throws {
        let (host, visual) = makeHost()
        try await load(host, SampleDrawings.eye.source)
        host.attach(to: visual)
        await step(host, at: 1)
        let views = host.canvas.subviews
        host.detach()
        #expect(host.canvas.superview == nil)
        let other = NSView(frame: NSRect(x: 0, y: 0, width: 60, height: 60))
        host.attach(to: other)
        #expect(host.canvas.superview === other)
        #expect(host.canvas.subviews.map(ObjectIdentifier.init) == views.map(ObjectIdentifier.init))
    }

    @Test("images reach drawImage through the ImageProviding handles in the input")
    func images() async throws {
        let images = FakeImages()
        images.images[3] = solidImage(width: 8, height: 8, red: 0, green: 1, blue: 0)
        let (host, visual) = makeHost(images: images)
        let input = FakeInputSource()
        input.base.show = InputSnapshot.Show(elements: [
            .image(ImageHandle(id: 3, width: 8, height: 8), caption: nil,
                   colors: ImageColors(dominant: "#00ff00", palette: ["#00ff00"])),
        ])
        host.input = input
        try await load(host, """
            peek.frame((ctx, input) => { ctx.drawImage(input.show.elements[0].image, 0, 0, 100, 100); return false })
            """)
        host.attach(to: visual)
        await step(host, at: 1)
        guard case .draw(let view) = host.compositor.slots.first, let image = view.image else {
            Issue.record("expected a draw view with an image")
            return
        }
        let center = pixel(image, x: image.width / 2, y: image.height / 2)
        #expect(center.g > 200 && center.a == 255, "\(center)")
    }

    @Test("the runtime makes live hosts; the fallback-only host never runs scripts")
    func runtimeAndFallbackHost() async throws {
        let runtime = DrawingRuntime(glassMode: .frosted)
        #expect(runtime.glassMode == .frosted)
        let host = runtime.makeHost(for: key, initial: "D", images: FakeImages())
        #expect(host is DrawingHost)
        #expect(QuickJSInfo.version == "0.17.0")
        let expected: GlassMode = NSWindow.instancesRespond(to: NSSelectorFromString("_hasActiveAppearance")) ? .live : .frosted
        #expect(DrawingRuntime().glassMode == expected)

        let fallback = FallbackDrawingHost(key: key, initial: "D")
        await #expect(throws: DrawingFailure.self) {
            try await fallback.load(DrawingScript(key: key, sha256: "x", source: Data("peek.frame(()=>false)".utf8),
                                                  filename: "x.js"))
        }
        let container = NSView(frame: NSRect(x: 0, y: 0, width: 100, height: 100))
        fallback.attach(to: container)
        #expect(container.subviews.count == 1)
        #expect(fallback.isOverContent(unitPoint: CGPoint(x: 50, y: 50)))
        fallback.detach()
        #expect(container.subviews.isEmpty)
        #expect(FallbackVisual.initial(from: " dj ") == "D")
        #expect(FallbackVisual.initial(from: "") == "?")
    }
}
