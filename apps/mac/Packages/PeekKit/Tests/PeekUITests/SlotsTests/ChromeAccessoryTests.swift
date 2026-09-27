import CoreGraphics
import Foundation
import PeekCore
import Testing

@testable import PeekUI

/// peek 0.1.2 chrome: the "+N" badge and the `^` of a compact ask next to the down-arrow, the hidden controls row and the
/// "Esc again to dismiss" hint (contract §8.5).
@Suite("Chrome layout 0.1.2: +N badge, ^ expand, compact ask, Esc hint")
struct ChromeAccessoryTests {
    let measurer = SlotFixedWidthMeasurer()
    /// Smallest visual (Ø 96 pt), the 14" fixture (Ø 181 pt) and the largest (Ø 240 pt).
    static let screens = [CGRect(x: 0, y: 0, width: 800, height: 560), SlotFixtures.visibleFrame,
                          CGRect(x: 0, y: 0, width: 2560, height: 1410)]

    static let options: ChromeContent.Controls = .choice(options: [
        ChromeContent.Option(id: "keep", label: "Keep"), ChromeContent.Option(id: "delete", label: "Delete"),
    ], multiple: false)

    func compactAsk(waiting: Int, hint: Bool = true) -> ChromeContent {
        ChromeContent(question: "Delete old.zip?", controls: Self.options, waiting: waiting, askCollapsed: true, escHint: hint)
    }

    func distance(_ a: CGPoint, _ b: CGPoint) -> CGFloat { hypot(a.x - b.x, a.y - b.y) }

    func distance(_ rect: CGRect, _ point: CGPoint) -> CGFloat {
        let nearest = CGPoint(x: min(max(point.x, rect.minX), rect.maxX), y: min(max(point.y, rect.minY), rect.maxY))
        return distance(nearest, point)
    }

    /// Points spread over a circle's disc or a rect, to test against the other hit shapes.
    func samples(circle center: CGPoint, radius: CGFloat) -> [CGPoint] {
        var points = [center]
        for ring in [0.5, 0.95] as [CGFloat] {
            for step in 0..<12 {
                let angle = Double(step) * .pi / 6
                points.append(CGPoint(x: center.x + radius * ring * CGFloat(cos(angle)), y: center.y + radius * ring * CGFloat(sin(angle))))
            }
        }
        return points
    }

    func samples(rect: CGRect) -> [CGPoint] {
        (0...4).flatMap { i in (0...2).map { j in
            CGPoint(x: rect.minX + 1 + (rect.width - 2) * CGFloat(i) / 4, y: rect.minY + 1 + (rect.height - 2) * CGFloat(j) / 2)
        } }
    }

    @Test("badge and ^ sit inside the panel and clear of every button and hit shape: 8 positions × both modes × 3 visual sizes")
    func placementEverywhere() throws {
        for screen in Self.screens {
            for mode in DisplayMode.allCases {
                for slot in SlotIndex.allCases {
                    let slotLayout = SlotGeometry.layout(slot: slot, mode: mode, visibleFrame: screen)
                    for waiting in [5, 12] {
                        let chrome = ChromeLayout.compute(layout: slotLayout, content: compactAsk(waiting: waiting), measurer: measurer)
                        let b = chrome.buttons
                        let place = "slot \(slot) \(mode) screen \(Int(screen.width)) +\(waiting)"
                        let expand = try #require(b.expand, "\(place): ^ on a compact ask")
                        let badge = try #require(b.badgeRect, "\(place): badge when something waits")
                        let panel = CGRect(origin: .zero, size: slotLayout.panelSize)
                        #expect(panel.contains(CGRect(x: expand.x - b.downRadius, y: expand.y - b.downRadius,
                                                      width: 2 * b.downRadius, height: 2 * b.downRadius)), "\(place): ^ inside")
                        #expect(panel.contains(badge), "\(place): badge inside \(badge) of \(panel)")
                        // Clear of the buttons (the compact badge rides the down-arrow's rim by design).
                        #expect(distance(expand, b.mic) >= b.radius + b.downRadius, "\(place): ^ vs mic")
                        #expect(distance(expand, b.keyboard) >= b.radius + b.downRadius, "\(place): ^ vs keyboard")
                        #expect(distance(expand, b.down) >= 2 * b.downRadius, "\(place): ^ vs down")
                        #expect(distance(badge, b.mic) >= b.radius, "\(place): badge vs mic")
                        #expect(distance(badge, b.keyboard) >= b.radius, "\(place): badge vs keyboard")
                        if mode == .normal { #expect(distance(badge, b.down) >= b.downRadius, "\(place): badge vs down") }
                        #expect(distance(badge, expand) >= b.downRadius, "\(place): badge vs ^")
                        // Clear of the row, the question and every other hit shape.
                        var others: [HitShape] = chrome.items.map { .rect($0.frame) }
                        switch chrome.question?.background {
                        case .arc(let band)?: others.append(.arc(band))
                        case .rect(let rect)?: others.append(.rect(rect))
                        case nil: break
                        }
                        for point in samples(circle: expand, radius: b.downRadius) + samples(rect: badge) {
                            #expect(!others.contains { $0.contains(point) }, "\(place): overlaps chrome at \(point)")
                        }
                    }
                }
            }
        }
    }

    @Test("the badge is a capsule round(1.5 × downRadius) tall, at least as wide, and wider for +12 than +5")
    func badgeSize() throws {
        let slotLayout = SlotFixtures.layout(.bottom)
        let five = ChromeLayout.compute(layout: slotLayout, content: ChromeContent(elements: [.text("Hi")], waiting: 5), measurer: measurer)
        let twelve = ChromeLayout.compute(layout: slotLayout, content: ChromeContent(elements: [.text("Hi")], waiting: 12),
                                          measurer: measurer)
        let size5 = try #require(five.buttons.badgeSize)
        let size12 = try #require(twelve.buttons.badgeSize)
        #expect(size5.height == (1.5 * five.buttons.downRadius).rounded())
        #expect(size5.width >= size5.height)
        #expect(size12.width > size5.width)
        #expect(size12.height == size5.height)
        #expect(five.buttons.expand == nil, "no ^ on a show")
        let none = ChromeLayout.compute(layout: slotLayout, content: ChromeContent(elements: [.text("Hi")]), measurer: measurer)
        #expect(none.buttons.badge == nil && none.buttons.badgeSize == nil)
    }

    @Test("normal mode: ^ between the down-arrow and the mic, the badge between the down-arrow and the keyboard")
    func normalSides() throws {
        for slot in SlotIndex.allCases {
            let slotLayout = SlotFixtures.layout(slot)
            let b = ChromeLayout.compute(layout: slotLayout, content: compactAsk(waiting: 3), measurer: measurer).buttons
            let expand = try #require(b.expand)
            let badge = try #require(b.badge)
            #expect(distance(expand, b.mic) < distance(expand, b.keyboard), "slot \(slot)")
            #expect(distance(badge, b.keyboard) < distance(badge, b.mic), "slot \(slot)")
            let c = slotLayout.visualCenter
            let buttonDistance = distance(b.down, c)
            #expect(distance(expand, c) >= buttonDistance - 0.5 && distance(expand, c) <= buttonDistance + 12.5, "slot \(slot)")
            #expect(distance(badge, c) >= buttonDistance - 0.5 && distance(badge, c) <= buttonDistance + 12.5, "slot \(slot)")
        }
    }

    @Test("compact display mode: ^ one spacing left of the down-arrow; the badge on its top-trailing rim, chevron visible")
    func compactPlacement() throws {
        for screen in Self.screens {
            for slot in SlotIndex.allCases {
                let slotLayout = SlotGeometry.layout(slot: slot, mode: .compact, visibleFrame: screen)
                for waiting in [2, 12] {
                    let b = ChromeLayout.compute(layout: slotLayout, content: compactAsk(waiting: waiting), measurer: measurer).buttons
                    let place = "slot \(slot) screen \(Int(screen.width)) +\(waiting)"
                    let expand = try #require(b.expand)
                    let badge = try #require(b.badge)
                    let rect = try #require(b.badgeRect)
                    #expect(expand.y == b.down.y, "\(place)")
                    #expect(expand.x < b.down.x, "\(place)")
                    #expect(badge.x >= b.down.x - 0.5, "\(place): trailing side (or right below)")
                    #expect(distance(rect, b.down) < b.downRadius, "\(place): overlaps the down-arrow's rim")
                    #expect(distance(rect, b.down) >= 0.4 * b.downRadius, "\(place): the chevron stays visible")
                }
            }
        }
    }

    @Test("a compact ask hides its controls (no option, field or track hit shapes); the question stays; the hint is a row pill")
    func compactAskLayout() {
        for mode in DisplayMode.allCases {
            for slot in SlotIndex.allCases {
                let slotLayout = SlotFixtures.layout(slot, mode)
                let full = ChromeLayout.compute(layout: slotLayout, content: ChromeContent(question: "Delete old.zip?",
                                                                                          controls: Self.options),
                                                measurer: measurer)
                #expect(!full.optionItems.isEmpty)
                let compact = ChromeLayout.compute(layout: slotLayout, content: compactAsk(waiting: 0, hint: false), measurer: measurer)
                #expect(compact.optionItems.isEmpty, "slot \(slot) \(mode)")
                #expect(compact.items.isEmpty, "slot \(slot) \(mode)")
                #expect(compact.track == nil)
                #expect(compact.field == nil)
                #expect(compact.question != nil)
                #expect(compact.buttons.expand != nil)
                let hinted = ChromeLayout.compute(layout: slotLayout, content: compactAsk(waiting: 0), measurer: measurer)
                #expect(hinted.items.map(\.kind) == [.hint], "slot \(slot) \(mode)")
                #expect(hinted.items.first?.fullText == ChromeContent.escAgainHint)
                // A text ask's curved field goes too.
                let text = ChromeLayout.compute(
                    layout: slotLayout,
                    content: ChromeContent(question: "Name?", controls: .text(placeholder: nil), askCollapsed: true), measurer: measurer)
                #expect(text.field == nil && text.item(.field) == nil, "slot \(slot) \(mode)")
            }
        }
    }

    @Test("the ^ is an interactive target (the badge is not); hovering the question of a compact ask shows it as clickable")
    @MainActor
    func expandTarget() throws {
        let slotLayout = SlotFixtures.layout(.bottom)
        let chrome = ChromeLayout.compute(layout: slotLayout, content: compactAsk(waiting: 4), measurer: measurer)
        let expand = try #require(chrome.buttons.expand)
        #expect(chrome.target(at: expand) == .expand)
        #expect(chrome.isOverChrome(expand))
        let badge = try #require(chrome.buttons.badge)
        #expect(chrome.target(at: badge) != .expand)
        #expect(chrome.anchorBox(of: .expand) != nil)

        let model = SlotChromeModel(slotLayout: slotLayout, measurer: measurer)
        var expanded = 0
        model.actions.expand = { expanded += 1 }
        model.setContent(compactAsk(waiting: 4), askPayload: nil)
        #expect(model.isInteractive(.expand))
        #expect(model.isInteractive(.question))
        model.questionClicked()
        #expect(expanded == 1, "a click on the compact ask's question expands it")
        #expect(model.resolve("expand") == .expand)
    }
}
