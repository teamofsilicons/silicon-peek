import AppKit
import Foundation
import PeekCore
import Testing

@testable import PeekInput

@Suite("Pointer → drawing units")
struct PointerMathTests {
    let frame = CGRect(x: 1000, y: 500, width: 200, height: 200)  // AppKit, y up

    @Test("corners and centre of the visual square map to 0…100, y down")
    func corners() throws {
        #expect(PointerMath.units(fromScreen: CGPoint(x: 1000, y: 700), visualFrameOnScreen: frame) == CGPoint(x: 0, y: 0))
        #expect(PointerMath.units(fromScreen: CGPoint(x: 1200, y: 500), visualFrameOnScreen: frame) == CGPoint(x: 100, y: 100))
        #expect(PointerMath.units(fromScreen: CGPoint(x: 1100, y: 600), visualFrameOnScreen: frame) == CGPoint(x: 50, y: 50))
        #expect(PointerMath.units(fromScreen: CGPoint(x: 900, y: 800), visualFrameOnScreen: frame) == CGPoint(x: -50, y: -50))
        #expect(PointerMath.units(fromScreen: .zero, visualFrameOnScreen: .zero) == nil)
    }

    @Test("dist, angle and inside are measured from (50, 50) against r = 50")
    func mouseFields() {
        let right = PointerMath.mouse(fromScreen: CGPoint(x: 1150, y: 600), visualFrameOnScreen: frame)
        #expect(right.x == 75 && right.y == 50)
        #expect(right.dist == 25)
        #expect(right.angle == 0)
        #expect(right.inside)
        let below = PointerMath.mouse(fromScreen: CGPoint(x: 1100, y: 520), visualFrameOnScreen: frame)
        #expect(close(below.angle, .pi / 2))  // y-down: below the centre is +π/2
        #expect(close(below.dist, 40))
        let corner = PointerMath.mouse(fromScreen: CGPoint(x: 1000, y: 700), visualFrameOnScreen: frame)
        #expect(!corner.inside)  // the square's corner lies outside the inscribed circle
        #expect(close(corner.dist, 50 * 2.0.squareRoot()))
        #expect(close(corner.angle, -3 * .pi / 4))
        #expect(PointerMath.mouse(fromScreen: .zero, visualFrameOnScreen: .zero) == .outside)
    }

    @Test("agrees with SlotLayout's own conversion for every slot")
    func matchesSlotGeometry() {
        let visible = CGRect(x: 0, y: 0, width: 1512, height: 944)
        for slot in SlotIndex.allCases {
            let layout = SlotGeometry.layout(slot: slot, mode: .normal, visibleFrame: visible)
            for point in [CGPoint(x: 700, y: 400), CGPoint(x: 12.5, y: 930), layout.visualFrameOnScreen.origin] {
                let ours = PointerMath.mouse(fromScreen: point, layout: layout)
                let theirs = layout.drawingUnits(fromScreen: point)
                #expect(close(ours.x, Double(theirs.x), 1e-9) && close(ours.y, Double(theirs.y), 1e-9))
            }
        }
    }
}

@Suite("Backdrop tone")
struct BackdropToneTests {
    @Test("Rec. 709 luminance on linearised sRGB")
    func luminance() {
        #expect(Backdrop.relativeLuminance(red: 1, green: 1, blue: 1) == 1)
        #expect(Backdrop.relativeLuminance(red: 0, green: 0, blue: 0) == 0)
        #expect(close(Backdrop.relativeLuminance(red: 1, green: 0, blue: 0), 0.2126))
        #expect(close(Backdrop.relativeLuminance(red: 0, green: 1, blue: 0), 0.7152))
        #expect(close(Backdrop.relativeLuminance(red: 0.5, green: 0.5, blue: 0.5), 0.21404, 1e-4))
    }

    @Test("hysteresis flips at 0.45 / 0.55, and ink is the opposite of the tone")
    func hysteresis() {
        var tone: BackdropTone? = nil
        var sequence: [BackdropTone] = []
        // Luminance 0.6 → 0.5 → 0.46 → 0.44 → 0.5 → 0.54 → 0.56
        for grey in [0.8, 0.735, 0.71, 0.70, 0.735, 0.76, 0.77] {
            let backdrop = Backdrop.sample(red: grey, green: grey, blue: grey, source: .wallpaper, previousTone: tone)
            tone = backdrop.tone
            sequence.append(backdrop.tone)
            #expect(backdrop.ink == (backdrop.tone == .dark ? "#ffffff" : "#000000"))
        }
        #expect(sequence == [.light, .light, .light, .dark, .dark, .dark, .light])
        let first = Backdrop.sample(red: 0.73, green: 0.73, blue: 0.73, source: .screen, previousTone: nil)
        #expect(first.tone == .dark)  // no history: plain 0.5 threshold
        #expect(first.color == "#bababa")
    }
}

@Suite("RGBA sampling")
struct RGBABitmapTests {
    @Test("region averages weigh partly covered pixels by area and ignore transparency")
    func averages() throws {
        // Left half red, right half blue; top row transparent.
        let image = try TestImages.image(width: 4, height: 3) { context in
            TestImages.fill(context, SRGBColor(red: 1, green: 0, blue: 0), CGRect(x: 0, y: 0, width: 2, height: 2))
            TestImages.fill(context, SRGBColor(red: 0, green: 0, blue: 1), CGRect(x: 2, y: 0, width: 2, height: 2))
        }
        let bitmap = try #require(RGBABitmap(image: image))
        #expect(bitmap.pixel(x: 0, y: 0).alpha == 0)  // top row (y-down) is the transparent one
        let whole = try #require(bitmap.averageColor(in: CGRect(x: 0, y: 0, width: 4, height: 3)))
        #expect(close(whole.color.red, 0.5, 0.01) && close(whole.color.blue, 0.5, 0.01))
        #expect(close(whole.opacity, 2.0 / 3.0, 0.01))
        let straddling = try #require(bitmap.averageColor(in: CGRect(x: 1.5, y: 1, width: 1, height: 2)))
        #expect(close(straddling.color.red, 0.5, 0.01))
        let mostlyRed = try #require(bitmap.averageColor(in: CGRect(x: 0, y: 1, width: 2.5, height: 2)))
        #expect(close(mostlyRed.color.red, 0.8, 0.01))
        #expect(bitmap.averageColor(in: CGRect(x: 0, y: 0, width: 4, height: 1)) == nil)
        #expect(bitmap.averageColor(in: CGRect(x: 10, y: 10, width: 2, height: 2)) == nil)
        #expect(SRGBColor(red: 1, green: 0.5, blue: 0).hex == "#ff8000")
    }
}

@Suite("Image palette")
struct PaletteTests {
    @Test("a 3:1 split gives the larger colour as dominant")
    func dominant() throws {
        let image = try TestImages.image(width: 64, height: 64) { context in
            TestImages.fill(context, SRGBColor(red: 0.1, green: 0.6, blue: 0.2), CGRect(x: 0, y: 0, width: 48, height: 64))
            TestImages.fill(context, SRGBColor(red: 0.9, green: 0.1, blue: 0.1), CGRect(x: 48, y: 0, width: 16, height: 64))
        }
        let colors = try #require(PaletteExtractor.colors(of: image))
        #expect(hexDistance(colors.dominant, SRGBColor(red: 0.1, green: 0.6, blue: 0.2).hex) <= 8)
        #expect(colors.palette.count >= 3 && colors.palette.count <= 5)
        #expect(colors.palette.contains { hexDistance($0, SRGBColor(red: 0.9, green: 0.1, blue: 0.1).hex) <= 8 })
        #expect(colors.palette[0] == colors.dominant)
    }

    @Test("four distinct quadrants give four palette colours, ordered by area")
    func quadrants() throws {
        let fills: [(SRGBColor, CGRect)] = [
            (SRGBColor(red: 1, green: 1, blue: 0), CGRect(x: 0, y: 0, width: 40, height: 64)),
            (SRGBColor(red: 0, green: 0, blue: 1), CGRect(x: 40, y: 0, width: 24, height: 40)),
            (SRGBColor(red: 0, green: 0, blue: 0), CGRect(x: 40, y: 40, width: 24, height: 14)),
            (SRGBColor(red: 1, green: 0, blue: 1), CGRect(x: 40, y: 54, width: 24, height: 10)),
        ]
        let image = try TestImages.image(width: 64, height: 64) { context in
            for (color, rect) in fills { TestImages.fill(context, color, rect) }
        }
        let colors = try #require(PaletteExtractor.colors(of: image))
        #expect(colors.palette.count == 4)
        for (index, (color, _)) in fills.enumerated() {
            #expect(hexDistance(colors.palette[index], color.hex) <= 8, "palette \(colors.palette) vs \(color.hex)")
        }
    }

    @Test("a flat image repeats its colour to fill three entries; the result is deterministic")
    func flatAndDeterministic() throws {
        let flat = try TestImages.image(width: 20, height: 20) { context in
            TestImages.fill(context, SRGBColor(red: 0.2, green: 0.4, blue: 0.6), CGRect(x: 0, y: 0, width: 20, height: 20))
        }
        let colors = try #require(PaletteExtractor.colors(of: flat))
        #expect(colors.palette == ["#336699", "#336699", "#336699"])
        #expect(colors.dominant == "#336699")

        let noisy = try TestImages.image(width: 64, height: 64) { context in
            var generator = SplitMix64(seed: 7)
            for x in 0..<16 {
                for y in 0..<16 {
                    let c = SRGBColor(red: Double(generator.next() % 256) / 255, green: Double(generator.next() % 256) / 255,
                                     blue: Double(generator.next() % 256) / 255)
                    TestImages.fill(context, c, CGRect(x: x * 4, y: y * 4, width: 4, height: 4))
                }
            }
        }
        let first = PaletteExtractor.colors(of: noisy)
        #expect(first == PaletteExtractor.colors(of: noisy))
        #expect((first?.palette.count ?? 0) >= 3 && (first?.palette.count ?? 0) <= 5)
    }

    @Test("a fully transparent image has no palette")
    func transparent() throws {
        let clear = try TestImages.image(width: 8, height: 8) { _ in }
        #expect(PaletteExtractor.colors(of: clear) == nil)
    }
}
