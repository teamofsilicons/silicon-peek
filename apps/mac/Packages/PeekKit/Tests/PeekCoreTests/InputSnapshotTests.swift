import Foundation
import Testing

@testable import PeekCore

@Suite("Input snapshot")
struct InputSnapshotTests {
    static let sample = InputSnapshot(
        t: 12.5, dt: 1.0 / 120,
        slot: .init(index: .bottom, facing: -.pi / 2),
        mode: .normal, appearance: .dark,
        backdrop: Backdrop.sample(red: 0.9, green: 0.8, blue: 0.7, source: .wallpaper, previousTone: nil),
        phase: .transcribing, hover: true, mouse: .init(x: 60, y: 50),
        speech: .init(text: "Now playing \"CO2\"", level: 0.42, progress: 0.3, done: false),
        mic: .init(level: 0.1),
        typing: .init(text: "yes"),
        show: .init(elements: [
            .text("Now playing"),
            .image(ImageHandle(id: 3, width: 512, height: 512), caption: "CO2 by Prateek Kuhad",
                   colors: ImageColors(dominant: "#c8352b", palette: ["#c8352b", "#f2e6c9", "#1c1c1c"])),
        ]),
        ask: .init(question: "Keep it?", type: .singleChoice,
                   options: [.init(id: "keep", label: "Keep"),
                             .init(id: "delete", label: "Delete", image: ImageHandle(id: 4, width: 64, height: 64),
                                   colors: ImageColors(dominant: "#ffffff", palette: ["#ffffff"]))],
                   value: .choice("keep"), highlight: "delete"),
        context: .testing, glass: .frosted)

    private func parsed(_ snapshot: InputSnapshot) throws -> JSONValue {
        try StrictJSON.parse(bytes: snapshot.jsonBytes())
    }

    @Test("the hand-written JSON is strict JSON with the amended A4 shape")
    func shape() throws {
        let json = try parsed(Self.sample)
        guard case .object(let fields) = json else {
            Issue.record("not an object")
            return
        }
        #expect(Set(fields.keys) == ["t", "dt", "slot", "mode", "appearance", "backdrop", "phase", "hover", "mouse", "speech",
                                     "mic", "typing", "show", "ask", "context", "glass"])
        #expect(json["slot"] == ["index": 5, "side": "bottom", "facing": .double(-.pi / 2)])
        #expect(json["phase"] == "transcribing")
        #expect(json["context"] == "testing")
        #expect(json["glass"] == "frosted")
        #expect(json["mouse"]?["inside"] == true)
        #expect(json["mouse"]?["dist"] == 10)
        #expect(json["show"]?["elements"]?.arrayValue?[1]["image"] == ["id": 3, "width": 512, "height": 512])
        #expect(json["ask"]?["options"]?.arrayValue?[0]["image"] == .null)
        #expect(json["ask"]?["value"] == "keep")
        #expect(json["ask"]?["min"] == nil)
    }

    @Test("no live word or transcript fields exist (BLUEPRINT §0.1, D9)")
    func noLiveWords() throws {
        let json = try parsed(Self.sample)
        #expect(json["speech"]?.objectValue.map { Set($0.keys) } == ["text", "level", "progress", "done"])
        #expect(json["mic"]?.objectValue.map { Set($0.keys) } == ["level"])
        let text = String(decoding: Self.sample.jsonBytes(), as: UTF8.self)
        #expect(!text.contains("\"word\""))
        #expect(!text.contains("transcript"))
    }

    @Test("the fast JSON decodes back into the same snapshot through Codable")
    func roundTrip() throws {
        let decoded = try JSONDecoder().decode(InputSnapshot.self, from: Data(Self.sample.jsonBytes()))
        #expect(decoded == Self.sample)
        // Codable's own encoding also decodes to the same value.
        let viaCodable = try JSONDecoder().decode(InputSnapshot.self, from: JSONEncoder().encode(Self.sample))
        #expect(viaCodable == Self.sample)
    }

    @Test("null members, slider asks and a hidden bubble encode as null / numbers")
    func minimal() throws {
        let snapshot = InputSnapshot(
            slot: .init(index: .top, facing: .pi / 2),
            ask: InputSnapshot.Ask(AskPayload(question: "Volume?", kind: .slider(SliderSpec(min: 0, max: 10, step: 1, defaultValue: 4))),
                                   value: .number(4), highlight: nil))
        let json = try parsed(snapshot)
        #expect(json["speech"] == .null)
        #expect(json["typing"] == .null)
        #expect(json["show"] == .null)
        #expect(json["phase"] == "hidden")
        #expect(json["backdrop"]?["source"] == "appearance")
        #expect(json["ask"]?["options"] == nil)
        #expect(json["ask"]?["min"] == 0)
        #expect(json["ask"]?["max"] == 10)
        #expect(json["ask"]?["step"] == 1)
        #expect(json["ask"]?["value"] == 4)
        #expect(json["ask"]?["highlight"] == .null)
        #expect(try JSONDecoder().decode(InputSnapshot.self, from: Data(snapshot.jsonBytes())) == snapshot)
    }

    @Test("non-finite numbers never produce invalid JSON")
    func nonFinite() throws {
        var snapshot = Self.sample
        snapshot.dt = .nan
        snapshot.mic.level = .infinity
        let json = try parsed(snapshot)
        #expect(json["dt"] == .null)
        #expect(json["mic"]?["level"] == .null)
    }

    @Test("strings with quotes, newlines and emoji survive")
    func strings() throws {
        var snapshot = Self.sample
        snapshot.typing = .init(text: "line1\nline2 \"quoted\" \\ 😀")
        #expect(try parsed(snapshot)["typing"]?["text"] == "line1\nline2 \"quoted\" \\ 😀")
    }

    @Test("ask views carry prepared image handles for options")
    func askFromPayload() {
        let payload = AskPayload(question: "Pick", kind: .singleChoice(options: [
            AskOption(id: "a", label: "A", image: "/c/a.png"), AskOption(id: "b", label: "B"),
        ]))
        let prepared = PreparedImage(handle: ImageHandle(id: 9, width: 10, height: 10),
                                     colors: ImageColors(dominant: "#000000", palette: ["#000000"]))
        let view = InputSnapshot.Ask(payload, value: nil, highlight: "a", images: ["/c/a.png": prepared])
        #expect(view.options?[0].image == prepared.handle)
        #expect(view.options?[1].image == nil)
        #expect(view.min == nil)
    }

    @Test("backdrop tone uses hysteresis at 0.45 / 0.55 and ink is the opposite of tone")
    func backdrop() {
        #expect(BackdropTone.next(previous: nil, luminance: 0.49) == .dark)
        #expect(BackdropTone.next(previous: .light, luminance: 0.46) == .light)
        #expect(BackdropTone.next(previous: .light, luminance: 0.44) == .dark)
        #expect(BackdropTone.next(previous: .dark, luminance: 0.54) == .dark)
        #expect(BackdropTone.next(previous: .dark, luminance: 0.56) == .light)
        let white = Backdrop.sample(red: 1, green: 1, blue: 1, source: .screen, previousTone: nil)
        #expect(white.tone == .light && white.ink == "#000000" && white.color == "#ffffff")
        #expect(abs(white.luminance - 1) < 1e-9)
        let black = Backdrop.sample(red: 0, green: 0, blue: 0, source: .screen, previousTone: nil)
        #expect(black.tone == .dark && black.ink == "#ffffff")
        #expect(abs(Backdrop.relativeLuminance(red: 0.5, green: 0.5, blue: 0.5) - 0.214) < 0.001)
    }

    @Test("drawing event payloads match visual.md A3 minus the word event")
    func drawingEvents() throws {
        #expect(DrawingEvent.enter.payloadJSON == nil)
        #expect(try StrictJSON.parse(bytes: #require(DrawingEvent.click(x: 12.5, y: 40, count: 2).payloadJSON))
            == ["x": 12.5, "y": 40, "count": 2])
        #expect(try StrictJSON.parse(bytes: #require(DrawingEvent.move(from: .right, to: .bottom).payloadJSON))
            == ["from": 3, "to": 5])
        #expect(try StrictJSON.parse(bytes: #require(DrawingEvent.answer(value: .choices(["a"]), via: .voice).payloadJSON))
            == ["value": ["a"], "via": "voice"])
        let send = try StrictJSON.parse(bytes: #require(DrawingEvent.send(show: Self.sample.show, ask: nil, speech: nil).payloadJSON))
        #expect(send["ask"] == .null)
        #expect(send["show"]?["elements"]?.arrayValue?.count == 2)
        #expect(Set(["enter", "leave", "send", "answer", "click", "move"]).contains(DrawingEvent.leave.name))
    }
}
