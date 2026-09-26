import CoreGraphics
import Foundation
import PeekCore
import SwiftUI
import Testing

@testable import PeekDrawing

@Suite("CSS colours")
struct CSSColorTests {
    @Test("hex, rgb(), hsl(), named and special colours parse like canvas")
    func parse() {
        #expect(CSSColor.parse("#f00") == RGBA(red: 1, green: 0, blue: 0, alpha: 1))
        #expect(CSSColor.parse("#FF000080")?.alpha == 128.0 / 255)
        #expect(CSSColor.parse("#1c1c1c") == RGBA(red: 28.0 / 255, green: 28.0 / 255, blue: 28.0 / 255, alpha: 1))
        #expect(CSSColor.parse("rgba(255,255,255,0.6)") == RGBA(red: 1, green: 1, blue: 1, alpha: 0.6))
        #expect(CSSColor.parse("rgb(100% 0% 0% / 50%)") == RGBA(red: 1, green: 0, blue: 0, alpha: 0.5))
        #expect(CSSColor.parse(" RebeccaPurple ") == RGBA(red: 0x66 / 255.0, green: 0x33 / 255.0, blue: 0x99 / 255.0, alpha: 1))
        #expect(CSSColor.parse("transparent") == .transparent)
        #expect(CSSColor.parse("currentColor") == .black)
        let green = CSSColor.parse("hsl(120deg 100% 50%)")
        #expect(green.map { abs($0.green - 1) < 1e-9 && $0.red < 1e-9 && $0.blue < 1e-9 } == true)
        let blue = CSSColor.parse("hsla(0.6667turn, 100%, 50%, 0.25)")
        #expect(blue.map { abs($0.blue - 1) < 1e-3 && $0.alpha == 0.25 } == true)
    }

    @Test("invalid colours are rejected")
    func invalid() {
        for text in ["", "blurple", "#12", "#ggg", "rgb(1,2)", "rgb(1 2 3 4)", "hsl(0, 50, 50)", "rgba(1,2,3,x)", "url(x)"] {
            #expect(CSSColor.parse(text) == nil, "\(text)")
        }
    }

    @Test("css strings round-trip for opaque and translucent colours")
    func cssString() {
        #expect(RGBA(red: 1, green: 0, blue: 0, alpha: 1).css == "#ff0000")
        #expect(RGBA(red: 0, green: 0, blue: 1, alpha: 0.5).css == "rgba(0, 0, 255, 0.5)")
    }
}

@Suite("fonts")
struct FontTests {
    @Test("CSS font shorthands map to the system families (visual.md A5)")
    func parse() throws {
        let semibold = try #require(FontSpec.parse("600 6px SF Pro"))
        #expect(semibold.family == .system && semibold.weight == 600 && semibold.size == 6 && !semibold.italic)
        let rounded = try #require(FontSpec.parse("italic bold 8px/10px \"SF Pro Rounded\", sans-serif"))
        #expect(rounded.family == .rounded && rounded.weight == 700 && rounded.italic && rounded.size == 8)
        #expect(FontSpec.parse("12px SF Mono")?.family == .mono)
        #expect(FontSpec.parse("300 9px 'New York'")?.family == .serif)
        #expect(FontSpec.parse("10px system-ui")?.family == .system)
        #expect(FontSpec.parse("6pt serif")?.size == 8)
        let unknown = try #require(FontSpec.parse("10px Comic Sans"))
        #expect(unknown.family == .system && unknown.unknownFamily == "Comic Sans")
        #expect(FontSpec.parse("bold SF Pro") == nil)
    }

    @Test("fonts are created at the unit size with the requested design and weight")
    func fonts() {
        let cache = FontCache()
        let regular = cache.font(for: FontSpec(family: .system, weight: 400, italic: false, size: 10, unknownFamily: nil))
        let bold = cache.font(for: FontSpec(family: .system, weight: 700, italic: false, size: 10, unknownFamily: nil))
        let rounded = cache.font(for: FontSpec(family: .rounded, weight: 400, italic: false, size: 20, unknownFamily: nil))
        let mono = cache.font(for: FontSpec(family: .mono, weight: 400, italic: false, size: 10, unknownFamily: nil))
        #expect(CTFontGetSize(regular) == 10)
        #expect(CTFontGetSize(rounded) == 20)
        #expect(CTFontGetSymbolicTraits(bold).contains(.traitBold))
        #expect(CTFontGetSymbolicTraits(mono).contains(.traitMonoSpace))
        #expect(CTFontCopyPostScriptName(rounded) as String != CTFontCopyPostScriptName(regular) as String)
        let wide = cache.measure("WWW", css: "10px SF Pro"), narrow = cache.measure("iii", css: "10px SF Pro")
        #expect(wide.width > narrow.width)
        #expect(wide.fontAscent > 5 && wide.fontDescent > 0)
    }
}

@Suite("samples and schedule")
struct SampleTests {
    private static let samplesDirectory = URL(fileURLWithPath: #filePath).resolvingSymlinksInPath()
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        .deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("Samples/drawings")

    @Test("the embedded samples match apps/mac/Samples/drawings (run embed.sh after editing them)")
    func embeddedMatchesFiles() throws {
        let files = try FileManager.default.contentsOfDirectory(atPath: Self.samplesDirectory.path)
            .filter { $0.hasSuffix(".js") }.sorted()
        #expect(files == SampleDrawings.all.map(\.filename).sorted())
        for sample in SampleDrawings.all {
            let text = try String(contentsOf: Self.samplesDirectory.appendingPathComponent(sample.filename), encoding: .utf8)
            #expect(text.trimmingCharacters(in: .newlines) == sample.source, "\(sample.filename) differs; run embed.sh")
        }
        #expect(SampleDrawings.simulation.filename == "deck.js")
        #expect(!SampleDrawings.all.contains { $0.source.contains("speech?.word") || $0.source.contains("transcript") })
    }

    @Test("the 90 validation frames cover every phase, mode, appearance, backdrop, ask type and context")
    func scheduleCoverage() {
        let steps = ValidationSchedule.steps(glass: .live)
        #expect(steps.count == 90)
        let inputs = steps.map(\.input)
        #expect(Set(inputs.map(\.phase)) == Set(Phase.allCases))
        #expect(Set(inputs.map(\.mode)) == Set(DisplayMode.allCases))
        #expect(Set(inputs.map(\.appearance)) == Set(Appearance.allCases))
        #expect(Set(inputs.map(\.backdrop.tone)) == Set(BackdropTone.allCases))
        #expect(Set(inputs.compactMap { $0.ask?.type }) == Set(AskType.allCases))
        #expect(Set(inputs.map(\.context)) == Set(InputContext.allCases))
        #expect(Set(inputs.map(\.glass)) == Set(GlassMode.allCases))
        #expect(inputs.contains { $0.hover } && inputs.contains { !$0.hover })
        #expect(inputs.contains { ($0.speech?.level ?? 0) > 0.5 } && inputs.contains { $0.mic.level > 0.5 })
        #expect(inputs.contains { $0.show?.elements.contains { if case .image = $0 { true } else { false } } == true })
        #expect(inputs.first?.dt == 0 && inputs.dropFirst().allSatisfy { $0.dt > 0 })
        let events = steps.flatMap(\.events).map(\.name)
        #expect(events.filter { $0 == "click" }.count == 1)
        #expect(events.filter { $0 == "move" }.count == 1)
        #expect(Set(events) == ["enter", "send", "click", "answer", "move", "leave"])
        // The slider value moves across its frames.
        let sliderValues = inputs.compactMap { input -> Double? in
            if case .number(let v)? = input.ask?.value, input.ask?.type == .slider { return v }
            return nil
        }
        #expect(Set(sliderValues).count == 10)
        #expect(ValidationSchedule.sampleImages().count == 3)
    }

    @Test("even-odd glass outlines are normalised so holes survive glassEffect's nonzero fill")
    func unitPathShape() {
        let path = CGMutablePath()
        path.addEllipse(in: CGRect(x: 10, y: 10, width: 80, height: 80))
        path.addEllipse(in: CGRect(x: 40, y: 40, width: 20, height: 20))
        let shape = UnitPathShape(cgPath: path, rule: .evenOdd)
        let scaled = shape.path(in: CGRect(x: 0, y: 0, width: 200, height: 200))
        #expect(!scaled.contains(CGPoint(x: 100, y: 100), eoFill: false))  // the hole, even under nonzero
        #expect(scaled.contains(CGPoint(x: 100, y: 40), eoFill: false))
        #expect(scaled.boundingRect.width == 160)
    }
}
