import CoreGraphics
import Foundation
import PeekCore
import Testing

@testable import PeekUI

@Suite("Esc router rules and the pre-warm's off-screen frame (pure)")
struct EscapeRoutingTests {
    typealias C = EscapeRouting.Candidate

    @Test("Esc is needed for a popup, a grace window (strictly before its end), hover on a bubble, or an open Esc window")
    func needsEsc() {
        #expect(!EscapeRouting.needsEsc([], now: 0))
        #expect(EscapeRouting.needsEsc([C(slot: .top, popupOpen: true)], now: 0))
        let fresh = C(slot: .top, onScreen: true, graceUntil: 3, slidInAt: 0)
        #expect(EscapeRouting.needsEsc([fresh], now: 2.999))
        #expect(!EscapeRouting.needsEsc([fresh], now: 3.0), "never longer than 3 s")
        #expect(!EscapeRouting.needsEsc([C(slot: .top, onScreen: false, graceUntil: 3)], now: 1), "a leaving bubble has no grace")
        #expect(EscapeRouting.needsEsc([C(slot: .top, onScreen: true, hovered: true)], now: 10))
        #expect(!EscapeRouting.needsEsc([C(slot: .top, onScreen: false, hovered: true)], now: 10))
        #expect(EscapeRouting.needsEsc([C(slot: .top, armedUntil: 10.4)], now: 10.3))
        #expect(!EscapeRouting.needsEsc([C(slot: .top, armedUntil: 10.4)], now: 10.4))
    }

    @Test("a press goes to the popup, else the latest Esc window, else the hovered bubble, else the newest bubble")
    func targets() {
        let old = C(slot: .bottom, onScreen: true, slidInAt: 1)
        let new = C(slot: .right, onScreen: true, slidInAt: 5)
        #expect(EscapeRouting.target([old, new], now: 6) == .bubble(.right))
        var hovered = old
        hovered.hovered = true
        #expect(EscapeRouting.target([hovered, new], now: 6) == .bubble(.bottom))
        let armedShort = C(slot: .left, onScreen: false, armedUntil: 6.3, slidInAt: 0)
        let armedLong = C(slot: .top, onScreen: true, armedUntil: 7.5, slidInAt: 0)
        #expect(EscapeRouting.target([hovered, new, armedShort], now: 6) == .bubble(.left))
        #expect(EscapeRouting.target([hovered, new, armedShort, armedLong], now: 6) == .bubble(.top))
        #expect(EscapeRouting.target([hovered, new, armedShort], now: 6.4) == .bubble(.bottom), "a closed window no longer counts")
        let popup = C(slot: .topLeft, popupOpen: true)
        #expect(EscapeRouting.target([hovered, new, armedLong, popup], now: 6) == .popup(.topLeft))
        #expect(EscapeRouting.target([C(slot: .top, onScreen: false, slidInAt: 3)], now: 4) == nil)
    }

    @Test("an Esc typed in a key panel: popup, then an open Esc window elsewhere, else the panel's own bubble")
    func keyPanelTargets() {
        let typing = C(slot: .bottom, onScreen: true, slidInAt: 1)
        let newer = C(slot: .right, onScreen: true, slidInAt: 5)
        #expect(EscapeRouting.keyPanelTarget([typing, newer], keySlot: .bottom, now: 6) == .bubble(.bottom))
        let armed = C(slot: .right, onScreen: true, armedUntil: 6.4, slidInAt: 5)
        #expect(EscapeRouting.keyPanelTarget([typing, armed], keySlot: .bottom, now: 6) == .bubble(.right))
        #expect(EscapeRouting.keyPanelTarget([typing, C(slot: .top, popupOpen: true)], keySlot: .bottom, now: 6) == .popup(.top))
    }

    @Test("the next deadline is the earliest grace or window end still ahead; priority orders several clients")
    func deadlinesAndPriority() {
        let a = C(slot: .bottom, onScreen: true, graceUntil: 3, slidInAt: 0)
        let b = C(slot: .right, onScreen: false, graceUntil: 2, armedUntil: 5.4, slidInAt: 1)
        #expect(EscapeRouting.nextDeadline([a, b], now: 1) == 3, "a leaving bubble's grace does not count")
        #expect(EscapeRouting.nextDeadline([a, b], now: 3.5) == 5.4)
        #expect(EscapeRouting.nextDeadline([a, b], now: 6) == nil)
        #expect(EscapeRouting.priority([C(slot: .top, popupOpen: true)], now: 0) == .infinity)
        #expect(EscapeRouting.priority([b], now: 5) > EscapeRouting.priority([C(slot: .top, onScreen: true, hovered: true)], now: 5))
        #expect(EscapeRouting.priority([a], now: 1) == 0)
        #expect(EscapeRouting.priority([], now: 1) == -.infinity)
    }

    @Test("the off-screen pre-warm frame never intersects a screen, for single and multi-display arrangements")
    func offScreenFrame() {
        let laptop = CGRect(x: 0, y: 0, width: 1512, height: 982)
        let arrangements: [[CGRect]] = [
            [laptop],
            [laptop, CGRect(x: 1512, y: -200, width: 2560, height: 1440)],  // external to the right
            [laptop, CGRect(x: -1920, y: 0, width: 1920, height: 1080)],  // external to the left
            [laptop, CGRect(x: 0, y: 982, width: 2560, height: 1440)],  // external above
            [laptop, CGRect(x: -300, y: -1080, width: 1920, height: 1080)],  // external below
        ]
        for screens in arrangements {
            for slot in SlotIndex.allCases {
                let layout = SlotGeometry.layout(slot: slot, mode: .normal, visibleFrame: screens[0].insetBy(dx: 0, dy: 20))
                let hidden = layout.hiddenOffset
                let frame = PrewarmStrategy.offScreenFrame(for: layout.panelFrame, outward: CGVector(dx: hidden.dx, dy: -hidden.dy),
                                                           screens: screens)
                #expect(!screens.contains { $0.intersects(frame) }, "slot \(slot) screens \(screens)")
                #expect(frame.size == layout.panelFrame.size)
            }
        }
        // No direction at all: the far-away fallback.
        let stuck = PrewarmStrategy.offScreenFrame(for: CGRect(x: 10, y: 10, width: 100, height: 100), outward: .zero,
                                                   screens: [laptop])
        #expect(stuck.minX == -20_000)
    }
}
