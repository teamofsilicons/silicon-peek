import AppKit
import Carbon.HIToolbox
import CoreGraphics
import Foundation
import PeekCore
import Testing

@testable import PeekUI

@Suite("Keys, typed answers, hotkeys and screens")
struct SlotInputTests {
    // MARK: Key classification (§8.5)

    func classify(_ characters: String?, keyCode: UInt16 = 0, cmd: Bool = false, _ context: KeyContext = .idle) -> KeyCommand {
        KeyClassifier.classify(characters: characters, charactersIgnoringModifiers: characters, keyCode: keyCode,
                               commandOrControl: cmd, context: context)
    }

    @Test("after a summon: \\ talks, a printable key types (seeded), Esc cancels, Return submits")
    func summonedKeys() {
        #expect(classify("\\", keyCode: 0x2A) == .voice)
        #expect(classify("h", keyCode: 4) == .type("h"))
        #expect(classify("é") == .type("é"))
        #expect(classify(nil, keyCode: KeyClassifier.escape) == .cancel)
        #expect(classify("\r", keyCode: KeyClassifier.returnKey) == .submit)
        #expect(classify("\u{F702}", keyCode: KeyClassifier.leftArrow) == .move(-1))
        #expect(classify("\u{F703}", keyCode: KeyClassifier.rightArrow) == .move(1))
        #expect(classify(" ", keyCode: KeyClassifier.space) == .toggle)
        #expect(classify("\u{F704}", keyCode: 0x7A) == .pass, "function keys are not text")
        #expect(classify("v", keyCode: 9, cmd: true) == .pass, "⌘V is a shortcut, not a character")
    }

    @Test("\\ is matched by character, so non-US layouts work")
    func backslashByCharacter() {
        // On a German layout ⌥⇧7 produces "\" with a different key code.
        #expect(KeyClassifier.classify(characters: "\\", charactersIgnoringModifiers: "\\", keyCode: 0x1A,
                                       commandOrControl: false, context: .idle) == .voice)
    }

    @Test("typing: only Esc is taken (and not while an input method composes); listening: \\ or Return stops")
    func modes() {
        #expect(classify("a", .typing(composing: false)) == .pass)
        #expect(classify("\r", keyCode: KeyClassifier.returnKey, .typing(composing: false)) == .pass)
        #expect(classify(nil, keyCode: KeyClassifier.escape, .typing(composing: false)) == .cancel)
        #expect(classify(nil, keyCode: KeyClassifier.escape, .typing(composing: true)) == .pass)
        #expect(classify("\\", .listening) == .submit)
        #expect(classify("\r", keyCode: KeyClassifier.returnKey, .listening) == .submit)
        #expect(classify("\u{3}", keyCode: KeyClassifier.keypadEnter, .listening) == .submit)
        #expect(classify("a", .listening) == .pass)
        #expect(classify("a", .busy) == .pass)
        #expect(classify(nil, keyCode: KeyClassifier.escape, .busy) == .cancel)
    }

    // MARK: Typed answers

    let options = [AskOption(id: "keep", label: "Keep"), AskOption(id: "delete", label: "Delete it"),
                   AskOption(id: "later", label: "Ask me later")]

    @Test("single choice: label, id, number, ordinal, prefix, whole word; ambiguity is refused")
    func singleChoiceMatching() {
        let ask = AskPayload(question: "Q?", kind: .singleChoice(options: options))
        #expect(TypedAnswerMatcher.match("Keep", ask: ask) == .choice("keep"))
        #expect(TypedAnswerMatcher.match("  delete IT ", ask: ask) == .choice("delete"))
        #expect(TypedAnswerMatcher.match("later", ask: ask) == .choice("later"))
        #expect(TypedAnswerMatcher.match("2", ask: ask) == .choice("delete"))
        #expect(TypedAnswerMatcher.match("#3", ask: ask) == .choice("later"))
        #expect(TypedAnswerMatcher.match("the second one", ask: ask) == .choice("delete"))
        #expect(TypedAnswerMatcher.match("option 1", ask: ask) == .choice("keep"))
        #expect(TypedAnswerMatcher.match("the last one", ask: ask) == .choice("later"))
        #expect(TypedAnswerMatcher.match("one", ask: ask) == .choice("keep"))
        #expect(TypedAnswerMatcher.match("del", ask: ask) == .choice("delete"))
        #expect(TypedAnswerMatcher.match("Délété it", ask: ask) == .choice("delete"), "accents are ignored")
        #expect(TypedAnswerMatcher.match("7", ask: ask) == nil)
        #expect(TypedAnswerMatcher.match("maybe", ask: ask) == nil)
        #expect(TypedAnswerMatcher.match("", ask: ask) == nil)
        let ambiguous = AskPayload(question: "Q?", kind: .singleChoice(options: [
            AskOption(id: "a", label: "Red apple"), AskOption(id: "b", label: "Red pear"),
        ]))
        #expect(TypedAnswerMatcher.match("red", ask: ambiguous) == nil)
    }

    @Test("multiple choice: lists with commas and 'and', inside min…max, in option order")
    func multipleChoiceMatching() {
        let ask = AskPayload(question: "Q?", kind: .multipleChoice(options: options, min: 1, max: 2))
        #expect(TypedAnswerMatcher.match("later and keep", ask: ask) == .choices(["keep", "later"]))
        #expect(TypedAnswerMatcher.match("1, 2", ask: ask) == .choices(["keep", "delete"]))
        #expect(TypedAnswerMatcher.match("1, 2, 3", ask: ask) == nil, "more than max")
        #expect(TypedAnswerMatcher.match("keep and nonsense", ask: ask) == nil)
    }

    @Test("slider and range: numbers inside the bounds, snapped to the step")
    func numbers() {
        let slider = AskPayload(question: "Q?", kind: .slider(SliderSpec(min: 0, max: 100, step: 5)))
        #expect(TypedAnswerMatcher.match("42", ask: slider) == .number(40))
        #expect(TypedAnswerMatcher.match("set it to 73%", ask: slider) == .number(75))
        #expect(TypedAnswerMatcher.match("150", ask: slider) == nil)
        #expect(TypedAnswerMatcher.match("loud", ask: slider) == nil)
        let fine = AskPayload(question: "Q?", kind: .slider(SliderSpec(min: -1, max: 1, step: 0.1)))
        #expect(TypedAnswerMatcher.match("-0.3", ask: fine) == .number(-0.3))
        #expect(TypedAnswerMatcher.match("0.30000000004", ask: fine) == .number(0.3))
        let range = AskPayload(question: "Q?", kind: .range(RangeSpec(min: 0, max: 1000, step: 1)))
        #expect(TypedAnswerMatcher.match("20-80", ask: range) == .range(lower: 20, upper: 80))
        #expect(TypedAnswerMatcher.match("between 900 and 1,000", ask: range) == .range(lower: 900, upper: 1000))
        #expect(TypedAnswerMatcher.match("80 to 20", ask: range) == .range(lower: 20, upper: 80))
        #expect(TypedAnswerMatcher.match("20", ask: range) == nil)
        let text = AskPayload(question: "Q?", kind: .text(placeholder: nil, maxLength: 500))
        #expect(TypedAnswerMatcher.match(" hi ", ask: text) == .text("hi"))
    }

    @Test("step decimals and snapping")
    func snapping() {
        #expect(TypedAnswerMatcher.decimalPlaces(of: 1) == 0)
        #expect(TypedAnswerMatcher.decimalPlaces(of: 0.25) == 2)
        #expect(TypedAnswerMatcher.decimalPlaces(of: 0.1) == 1)
        #expect(TypedAnswerMatcher.clampAndSnap(0.1 + 0.2, min: 0, max: 1, step: 0.1) == 0.3)
        #expect(TypedAnswerMatcher.clampAndSnap(-5, min: 0, max: 1, step: 0.1) == 0)
        #expect(TypedAnswerMatcher.clampAndSnap(7.4, min: 1, max: 10, step: 3) == 7)
    }

    // MARK: Hotkeys and screens

    @Test("hotkeys are registered only for occupied slots that ask for one")
    func hotkeyPlan() {
        let table = [
            SlotState(index: .bottom, actorID: "si:dj", orgID: "tos"),
            SlotState(index: .top, context: .testing(environmentID: "e"), actorID: "si:t", orgID: "tos"),
            SlotState(index: .left, actorID: "si:quiet", orgID: "tos", hotkey: false),
            SlotState(index: .right, context: .simulation, actorID: "si:simulation", orgID: "simulation"),
            SlotState(index: .bottom, context: .testing(environmentID: "e"), actorID: "si:dj", orgID: "tos"),
        ]
        #expect(HotKeyPlan.slots(for: table) == [.bottom, .top])
        #expect(HotKeyPlan.label(.bottom, .cmd) == "⌘5")
        #expect(HotKeyPlan.label(.topLeft, .ctrlOpt) == "⌃⌥8")
        let taken = HotKeyPlan.failureMessage(.right, .cmd, status: OSStatus(eventHotKeyExistsErr))
        #expect(taken.contains("⌘3"))
        #expect(taken.contains("already taken"))
        #expect(taken.contains("Settings"))
    }

    @Test("slot keys are the physical digits 1…8 (kVK_ANSI_1…8)")
    func hotkeyCodes() {
        #expect(SlotIndex.allCases.map(\.hotKeyCode) == [
            UInt32(kVK_ANSI_1), UInt32(kVK_ANSI_2), UInt32(kVK_ANSI_3), UInt32(kVK_ANSI_4),
            UInt32(kVK_ANSI_5), UInt32(kVK_ANSI_6), UInt32(kVK_ANSI_7), UInt32(kVK_ANSI_8),
        ])
        #expect(HotkeyModifier.cmd.carbonMask == UInt32(cmdKey))
        #expect(HotkeyModifier.ctrlOptCmd.carbonMask == UInt32(cmdKey | optionKey | controlKey))
    }

    @Test("display policy: the menu-bar screen by default, the screen under the pointer when following it")
    func screens() {
        let frames = [CGRect(x: 0, y: 0, width: 1512, height: 982), CGRect(x: 1512, y: -200, width: 2560, height: 1440)]
        #expect(ScreenPolicy.screenIndex(for: .main, pointer: CGPoint(x: 2000, y: 100), frames: frames) == 0)
        #expect(ScreenPolicy.screenIndex(for: .pointer, pointer: CGPoint(x: 2000, y: 100), frames: frames) == 1)
        #expect(ScreenPolicy.screenIndex(for: .pointer, pointer: CGPoint(x: 100, y: 100), frames: frames) == 0)
        #expect(ScreenPolicy.screenIndex(for: .pointer, pointer: CGPoint(x: -500, y: -500), frames: frames) == 0,
                "a pointer outside every screen falls back to the menu-bar screen")
        #expect(ScreenPolicy.screenIndex(for: .main, pointer: .zero, frames: []) == nil)
    }

    @Test("the private glass selector is checked at run time")
    func glassSelfCheck() {
        let mode = GlassSupport.selfCheck()
        let responds = NSWindow.instancesRespond(to: NSSelectorFromString("_hasActiveAppearance"))
        #expect(mode == (responds ? .live : .frosted))
    }

    @Test("both glass modes can overlay another app's full-screen Space without activating Peek")
    @MainActor
    func fullScreenOverlay() {
        _ = NSApplication.shared
        for panel in [SlotPanel(contentRect: .zero), ActiveLookSlotPanel(contentRect: .zero)] {
            #expect(panel.collectionBehavior.contains(.canJoinAllApplications))
            #expect(panel.collectionBehavior.contains(.canJoinAllSpaces))
            #expect(panel.styleMask.contains(.nonactivatingPanel))
            #expect(!panel.canBecomeMain)
            panel.close()
        }
    }
}
