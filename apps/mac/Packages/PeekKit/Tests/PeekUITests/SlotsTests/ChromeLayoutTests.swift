import CoreGraphics
import Foundation
import PeekCore
import Testing

@testable import PeekUI

@Suite("Chrome layout: arc placement, widths, controls, buttons, hit testing")
struct ChromeLayoutTests {
    typealias F = SlotFixtures
    let measurer = SlotFixedWidthMeasurer()

    func compute(_ slot: SlotIndex, _ content: ChromeContent, mode: DisplayMode = .normal) -> ChromeLayout {
        ChromeLayout.compute(layout: F.layout(slot, mode), content: content, measurer: measurer)
    }

    func along(_ item: ChromeLayout.Item, _ arc: InfoArc) -> CGFloat {
        arc.alongAndDepth(of: item.frame.size).along
    }

    // MARK: Row placement

    @Test("row placement matches PeekCore's InfoArc.place for the same element sizes, on every slot")
    func matchesInfoArcPlace() throws {
        for slot in SlotIndex.allCases {
            let slotLayout = F.layout(slot)
            let arc = try #require(slotLayout.arc)
            let third = arc.maxElementAlongExtent
            let content = ChromeContent(elements: (0..<3).map { .image(key: "i\($0)", aspect: 1, caption: nil) })
            let chrome = compute(slot, content)
            let expected = arc.place(Array(repeating: ElementRequest(size: CGSize(width: third, height: third), fit: .scale),
                                           count: 3))
            #expect(chrome.items.count == 3)
            for (item, placement) in zip(chrome.items, expected) {
                #expect(abs(item.frame.center.x - placement.center.x) < 1.01, "slot \(slot)")
                #expect(abs(item.frame.center.y - placement.center.y) < 1.01, "slot \(slot)")
                #expect(abs(item.frame.rotation - placement.rotation) < 0.005, "slot \(slot)")
            }
        }
    }

    @Test("no show element is wider than a third of the arc (text columns on the left/right arcs may run taller)")
    func elementsCappedAtAThird() throws {
        let long = String(repeating: "word ", count: 32).trimmingCharacters(in: .whitespaces)  // 159 chars
        let content = ChromeContent(elements: [
            .text(long), .image(key: "wide", aspect: 4, caption: "A very long caption that will not fit"), .text("2022"),
        ])
        for slot in SlotIndex.allCases {
            let arc = try #require(F.layout(slot).arc)
            let chrome = compute(slot, content)
            #expect(chrome.items.count == 3)
            for item in chrome.items {
                if slot.side.isVertical, case .textPill = item.kind {
                    // "Narrow and tall": on the vertical arcs tall runs along the arc, never past it.
                    #expect(along(item, arc) <= arc.length, "slot \(slot) item \(item.kind)")
                    #expect(item.frame.size.width <= arc.maxElementAlongExtent + 0.5, "slot \(slot)")
                } else {
                    #expect(along(item, arc) <= arc.maxElementAlongExtent + 0.5, "slot \(slot) item \(item.kind)")
                }
            }
            // The long text wraps instead of running off.
            if case .textPill(_, let lines) = chrome.items[0].kind { #expect(lines > 1) }
        }
    }

    @Test("long text runs narrow and tall, grows away from the edge and stays inside the panel at all 8 positions")
    func longTextColumn() throws {
        let long = String(repeating: "word ", count: 32).trimmingCharacters(in: .whitespaces)  // 159 chars
        for slot in SlotIndex.allCases {
            let slotLayout = F.layout(slot)
            let arc = try #require(slotLayout.arc)
            let chrome = compute(slot, ChromeContent(elements: [.text(long)]))
            let item = try #require(chrome.items.first)
            guard case .textPill(_, let lines) = item.kind else {
                Issue.record("expected a text pill on slot \(slot)")
                continue
            }
            #expect(lines >= 5, "slot \(slot): \(lines) lines")
            #expect(item.frame.size.width <= ChromeMetrics.columnMaxWidth + 2 * ChromeMetrics.pillPadding + 2, "narrow, slot \(slot)")
            #expect(item.frame.size.height >= item.frame.size.width * 0.7, "tall, slot \(slot): \(item.frame.size)")
            #expect(!item.truncated, "all of it fits, slot \(slot)")
            let panel = CGRect(origin: .zero, size: slotLayout.panelSize).insetBy(dx: 3, dy: 3)
            #expect(panel.contains(item.frame.boundingBox), "inside the panel, slot \(slot)")
            #expect(abs(wrappedAngle(item.frame.rotation)) <= ChromeMetrics.columnMaxTilt + 1e-6 || slot.side.isVertical,
                    "a column tilts at most 15°, slot \(slot)")
            // It grows away from the screen edge: every corner is outside the baseline, toward the screen centre.
            for corner in item.frame.corners {
                #expect(arc.radialDistance(of: corner) >= arc.radius - 0.5, "slot \(slot)")
            }
        }
        // Smaller and larger screens: still inside the panel, cut (and flagged) rather than spilling over.
        for frame in [CGRect(x: 0, y: 0, width: 1280, height: 777), CGRect(x: 0, y: 0, width: 1728, height: 1079),
                      CGRect(x: 0, y: 0, width: 2560, height: 1415)] {
            for slot in SlotIndex.allCases {
                let slotLayout = SlotGeometry.layout(slot: slot, mode: .normal, visibleFrame: frame)
                let content = ChromeContent(elements: [.text(long), .image(key: "c", aspect: 1, caption: "Neon Tide"),
                                                       .text(long)])
                let chrome = ChromeLayout.compute(layout: slotLayout, content: content, measurer: measurer)
                let panel = CGRect(origin: .zero, size: slotLayout.panelSize).insetBy(dx: 3, dy: 3)
                for item in chrome.items {
                    #expect(panel.contains(item.frame.boundingBox), "\(frame.size) slot \(slot)")
                    if case .textPill(_, let lines) = item.kind, !item.truncated {
                        #expect(lines >= 3, "\(frame.size) slot \(slot)")
                    }
                }
            }
        }
        // Short text stays a single-line pill.
        let short = compute(.bottom, ChromeContent(elements: [.text("Standup in 5")]))
        if case .textPill(_, let lines)? = short.items.first?.kind { #expect(lines == 1) }
    }

    @Test("cut-short text is flagged with its full text; static text expands on click, options only reveal on hover")
    func truncationFlags() throws {
        let label = "Move all three to the archive drive now"
        let options = [label, "Delete them permanently right now", "Keep"].enumerated().map {
            ChromeContent.Option(id: "\($0.offset)", label: $0.element)
        }
        let ask = compute(.bottom, ChromeContent(question: "Clean up?", controls: .choice(options: options, multiple: false)))
        let first = try #require(ask.item(identity: "option-0"))
        #expect(first.truncated)
        #expect(ask.hiddenText(of: .item("option-0")) == label)
        #expect(!ask.isExpandable(.item("option-0")), "a click on an option answers")
        #expect(ask.hiddenText(of: .item("option-2")) == nil, "\"Keep\" fits")
        #expect(ask.target(at: first.frame.center) == .item("option-0"))
        #expect(ask.target(at: ask.buttons.mic) == .mic)
        #expect(ask.target(at: CGPoint(x: 1, y: 1)) == nil)

        let caption = String(repeating: "Deluxe ", count: 7).trimmingCharacters(in: .whitespaces)
        let show = compute(.top, ChromeContent(elements: [.image(key: "c", aspect: 1, caption: caption), .text("2022")]))
        #expect(show.item(identity: "image-0")?.truncated == true)
        #expect(show.isExpandable(.item("image-0")))
        #expect(!show.isExpandable(.item("text-1")))
        let measurer = SlotFixedWidthMeasurer()
        let panel = CGRect(origin: .zero, size: show.panelSize)
        let popup = try #require(ChromeOverlay.make(.popup, for: .item("image-0"), text: caption, layout: show, measurer: measurer))
        #expect(panel.contains(popup.frame))
        let tooltip = try #require(ChromeOverlay.make(.tooltip, for: .item("option-0"), text: label, layout: ask,
                                                      measurer: measurer))
        #expect(CGRect(origin: .zero, size: ask.panelSize).contains(tooltip.frame))
        #expect(!tooltip.frame.intersects(first.frame.boundingBox.insetBy(dx: 2, dy: 2)), "beside the option, not over it")

        // A question too long for two lines at the smallest size is cut and expandable.
        let longQuestion = String(repeating: "Should I really delete this? ", count: 6)
        let q = compute(.bottom, ChromeContent(question: longQuestion, controls: .choice(options: options, multiple: false)))
        #expect(q.question?.truncated == true)
        #expect(q.isExpandable(.question))
    }

    @Test("elements are flex-centred on the arc's midpoint in reading order")
    func flexCentred() throws {
        let chrome = compute(.bottom, ChromeContent(elements: [.text("one"), .text("two"), .text("six")]))
        let s = chrome.items.map(\.arcLength)
        #expect(s[0] < s[1] && s[1] < s[2])
        #expect(abs(s[0] + s[2]) < 0.5)
        #expect(abs(s[1]) < 0.5)
        // Reading order runs left to right on the bottom arc.
        #expect(chrome.items[0].frame.center.x < chrome.items[2].frame.center.x)
    }

    @Test("pills away from the apex rotate to the arc tangent (the rotated '2022' pill)")
    func pillsFollowTangent() throws {
        for slot in [SlotIndex.bottom, .top, .topRight, .bottomLeft] {
            let chrome = compute(slot, ChromeContent(elements: [.text("2022"), .text("CO2!"), .text("Kuhd")]))
            let r = chrome.items.map(\.frame.rotation)
            #expect(abs(r[0] - r[1]) > 0.05, "slot \(slot)")
            #expect(abs((r[0] - r[1]) + (r[2] - r[1])) < 0.02, "symmetric around the middle, slot \(slot)")
        }
        // On the left/right arcs elements stay upright and stack top to bottom.
        let left = compute(.left, ChromeContent(elements: [.text("a"), .text("b")]))
        #expect(left.items.allSatisfy { abs($0.frame.rotation) < 0.35 })
        #expect(left.items[0].frame.center.y < left.items[1].frame.center.y)
    }

    @Test("an image card is the image plus its caption pill, image above")
    func imageCard() throws {
        let chrome = compute(.bottom, ChromeContent(elements: [.image(key: "cover", aspect: 1, caption: "CO2 by Prateek Kuhad")]))
        let item = try #require(chrome.items.first)
        guard case .imageCard(_, let imageSize) = item.kind else {
            Issue.record("expected an image card, got \(item.kind)")
            return
        }
        #expect(abs(item.frame.size.height - (imageSize.height + ChromeMetrics.captionHeight + ChromeMetrics.captionGap)) < 1.01)
        #expect(imageSize.width <= item.frame.size.width)
    }

    @Test("the TEST badge leads the row")
    func badgeLeads() {
        let chrome = compute(.bottom, ChromeContent(badge: .test(name: "staging"), elements: [.text("hi")]))
        #expect(chrome.items.first?.kind == .badge)
        #expect(chrome.items[0].arcLength < chrome.items[1].arcLength)
        #expect(ChromeContent.Badge.test(name: "staging").text == "TEST · staging")
        #expect(ChromeContent.Badge.simulation.text == "SIMULATION")
    }

    // MARK: Ask controls

    @Test("six long options with images still fit inside the arc")
    func sixOptionsFit() throws {
        let options = (1...6).map { ChromeContent.Option(id: "\($0)", label: "Option number \($0) is long", imageKey: "img\($0)") }
        for slot in SlotIndex.allCases {
            let arc = try #require(F.layout(slot).arc)
            let chrome = compute(slot, ChromeContent(question: "Pick one", controls: .choice(options: options, multiple: false)))
            let items = chrome.optionItems
            #expect(items.count == 6)
            let total = items.reduce(CGFloat(0)) { $0 + along($1, arc) } + arc.gap * 5
            #expect(total <= arc.length + 1, "slot \(slot)")
        }
    }

    @Test("multiple choice ends with the ✓; single choice has none")
    func confirmPlacement() {
        let options = ["a", "b"].map { ChromeContent.Option(id: $0, label: $0) }
        let multiple = compute(.top, ChromeContent(question: "Q?", controls: .choice(options: options, multiple: true)))
        #expect(multiple.items.last?.kind == .confirm)
        let single = compute(.top, ChromeContent(question: "Q?", controls: .choice(options: options, multiple: false)))
        #expect(!single.items.contains { $0.kind == .confirm })
    }

    @Test("slider and range tracks take the whole arc; a pointer on the track maps back to its fraction")
    func scaleTrack() throws {
        for slot in SlotIndex.allCases {
            let arc = try #require(F.layout(slot).arc)
            let chrome = compute(slot, ChromeContent(question: "How loud?", controls: .scale(isRange: false)))
            let track = try #require(chrome.track)
            #expect(chrome.trackRole == .slider)
            #expect(track.length >= arc.length - ChromeMetrics.confirmSize - arc.gap - 1, "slot \(slot)")
            for t in [0.0, 0.25, 0.5, 0.9, 1.0] {
                #expect(abs(track.fraction(nearest: track.point(at: t)) - t) < 1e-6)
            }
            #expect(chrome.isOverChrome(track.point(at: 0.3)), "the track catches the pointer")
        }
        let range = compute(.bottom, ChromeContent(question: "Between?", controls: .scale(isRange: true)))
        #expect(range.trackRole == .range)
    }

    @Test("typing: a curved field band centred on the arc (top/bottom and corners), a straight one on the left/right")
    func typingField() throws {
        for slot in SlotIndex.allCases {
            let arc = try #require(F.layout(slot).arc)
            let chrome = compute(slot, ChromeContent(input: .typing))
            if slot.side.isVertical {
                let item = try #require(chrome.item(.field), "slot \(slot)")
                #expect(item.frame.size.width >= ChromeMetrics.fieldMinWidth)
                #expect(chrome.field == nil)
                continue
            }
            #expect(chrome.item(.field) == nil, "no straight field on slot \(slot)")
            let field = try #require(chrome.field, "slot \(slot)")
            #expect(field.arc.center == arc.center, "concentric with the arc, slot \(slot)")
            #expect(2 * field.halfLength >= ChromeMetrics.fieldMinWidth)
            #expect(2 * field.halfLength <= field.arc.length)
            #expect(field.textStart < field.textEnd && field.textEnd < field.submitS)
            #expect(chrome.isOverChrome(field.arc.apex), "the band catches the pointer")
            #expect(chrome.target(at: field.submitCenter) == .submit)
        }
    }

    @Test("a text ask's curved field sits right next to its question: question first in reading order")
    func textAskFieldNextToQuestion() throws {
        let content = ChromeContent(badge: .simulation, question: "What should I call tonight's playlist?",
                                    controls: .text(placeholder: "e.g. Late-night focus"))
        for slot in SlotIndex.allCases {
            let arc = try #require(F.layout(slot).arc)
            let chrome = compute(slot, content)
            let question = try #require(chrome.question, "slot \(slot)")
            if slot.side.isVertical {
                // Upright: the question block stacked right above the field, as wide as it.
                let field = try #require(chrome.item(.field), "slot \(slot)")
                guard case .rect(let block) = question.background else {
                    Issue.record("expected a block on slot \(slot)")
                    continue
                }
                #expect(block.boundingBox.maxY <= field.frame.boundingBox.minY + 0.5, "question above the field, slot \(slot)")
                #expect(field.frame.boundingBox.minY - block.boundingBox.maxY <= arc.gap + 1, "adjacent, slot \(slot)")
                continue
            }
            let field = try #require(chrome.field, "slot \(slot)")
            guard case .arc(let band) = question.background, case .arc(_, let upIsOutward) = question.lines[0].placement else {
                Issue.record("expected a curved question on slot \(slot)")
                continue
            }
            let gap = upIsOutward ? band.innerRadius - field.band.outerRadius : field.band.innerRadius - band.outerRadius
            #expect(gap >= 0 && gap <= ChromeMetrics.fieldQuestionGap + 0.5, "adjacent bands, slot \(slot): \(gap)")
            // The question reads first: it is the band higher on the screen.
            let questionY = question.lines[0].placement.arcApexY, fieldY = field.arc.apex.y
            #expect(questionY < fieldY, "question above the field on screen, slot \(slot)")
        }
    }

    @Test("listening: the waveform band fills the arc")
    func waveformFillsArc() throws {
        let arc = try #require(F.layout(.right).arc)
        let chrome = compute(.right, ChromeContent(input: .listening))
        let track = try #require(chrome.track)
        #expect(chrome.trackRole == .waveform)
        #expect(abs(track.length - arc.length) < 0.5)
    }

    // MARK: Question arc

    @Test("the question sits on its own arc outside every control, and short questions take one line")
    func questionOutsideControls() throws {
        let options = (1...3).map { ChromeContent.Option(id: "\($0)", label: "Opt \($0)", imageKey: "i\($0)") }
        for slot in SlotIndex.allCases where !slot.side.isVertical {
            let arc = try #require(F.layout(slot).arc)
            let chrome = compute(slot, ChromeContent(question: "Keep it?", controls: .choice(options: options, multiple: false)))
            let question = try #require(chrome.question)
            #expect(question.lines.count == 1)
            guard case .arc(let band) = question.background else {
                Issue.record("expected a curved band on slot \(slot)")
                continue
            }
            let rowOuter = chrome.items.flatMap(\.frame.corners).map { arc.radialDistance(of: $0) }.max() ?? 0
            #expect(band.innerRadius > rowOuter, "slot \(slot)")
            guard case .arc(let lineArc, _) = question.lines[0].placement else { continue }
            #expect(lineArc.center == arc.center)
            #expect(lineArc.radius > band.innerRadius && lineArc.radius < band.outerRadius)
        }
    }

    @Test("glyph tops point up the screen: away from the arc centre at the bottom, toward it at the top")
    func glyphOrientation() throws {
        let content = ChromeContent(question: "Keep it?", controls: .text(placeholder: nil))
        let bottom = try #require(compute(.bottom, content).question?.lines.first)
        let top = try #require(compute(.top, content).question?.lines.first)
        guard case .arc(_, let bottomUp) = bottom.placement, case .arc(_, let topUp) = top.placement else {
            Issue.record("expected curved lines")
            return
        }
        #expect(bottomUp)
        #expect(!topUp)
    }

    @Test("a long question wraps onto two concentric lines, the first one on top")
    func twoLineQuestion() throws {
        let question = String(repeating: "Should I really delete ", count: 4).prefix(80)
        let content = ChromeContent(question: String(question), controls: .scale(isRange: false))
        for slot in [SlotIndex.bottom, .top] {
            let lines = try #require(compute(slot, content).question?.lines)
            #expect(lines.count == 2, "slot \(slot)")
            guard case .arc(let first, _) = lines[0].placement, case .arc(let second, _) = lines[1].placement else { continue }
            let firstY = first.point(atArcLength: 0).y, secondY = second.point(atArcLength: 0).y
            #expect(firstY < secondY, "reading order runs down the screen on slot \(slot)")
        }
    }

    @Test("glyphs are set on the line's circle, in reading order")
    func glyphsOnArc() throws {
        let line = try #require(compute(.bottom, ChromeContent(question: "Keep it?", controls: .text(placeholder: nil)))
            .question?.lines.first)
        guard case .arc(let arc, _) = line.placement else {
            Issue.record("expected a curved line")
            return
        }
        let glyphs = ArcTextSetter.place(line.text, font: line.font, on: arc)
        #expect(glyphs.count == line.text.count)
        for glyph in glyphs {
            #expect(abs(arc.radialDistance(of: glyph.center) - arc.radius) < 0.01)
        }
        #expect(zip(glyphs, glyphs.dropFirst()).allSatisfy { $0.center.x < $1.center.x })
    }

    @Test("a notice takes the question's place")
    func noticeReplacesQuestion() throws {
        let question = try #require(compute(.bottom, ChromeContent(question: "Keep?", controls: .text(placeholder: nil),
                                                                    notice: "Didn't match an option — tap one or type")).question)
        #expect(question.isNotice)
        #expect(question.lines.map(\.text).joined(separator: " ").contains("Didn't match"))
    }

    // MARK: Buttons

    @Test("mic, keyboard and down-arrow sit in the visual's outer corners, inside the panel, apart from each other")
    func buttons() throws {
        for slot in SlotIndex.allCases {
            let slotLayout = F.layout(slot)
            let chrome = compute(slot, ChromeContent(elements: [.text("x")]))
            let buttons = chrome.buttons
            let c = slotLayout.visualCenter, r = slotLayout.visualRadius
            let inward = slot.side.inward
            let panel = CGRect(origin: .zero, size: slotLayout.panelSize)
            for point in [buttons.mic, buttons.keyboard, buttons.down] {
                #expect(hypot(point.x - c.x, point.y - c.y) >= r + buttons.radius, "clear of the visual, slot \(slot)")
                let outwardness = -(Double(point.x - c.x) * inward.dx + Double(point.y - c.y) * inward.dy)
                #expect(outwardness > 0, "on the edge side of the visual, slot \(slot)")
                #expect(panel.insetBy(dx: buttons.radius, dy: buttons.radius).contains(point), "inside the panel, slot \(slot)")
                #expect(!chrome.items.contains { $0.frame.contains(point) }, "not under an arc item, slot \(slot)")
            }
            #expect(hypot(buttons.mic.x - buttons.keyboard.x, buttons.mic.y - buttons.keyboard.y) > 2 * buttons.radius)
            if !slot.side.isVertical { #expect(buttons.mic.x < buttons.keyboard.x, "mic first in reading order, slot \(slot)") }
        }
        // The chevron points at the edge the bubble hides behind: down on the bottom slot, up on the top one.
        #expect(abs(compute(.bottom, ChromeContent()).buttons.downRotation) < 1e-9)
        #expect(abs(abs(compute(.top, ChromeContent()).buttons.downRotation) - .pi) < 1e-9)
    }

    // MARK: Hit testing

    @Test("clicks pass through empty panel space but not through pills, the question band or the buttons")
    func hitTesting() throws {
        let chrome = compute(.bottom, ChromeContent(question: "Keep?", controls: .choice(
            options: [.init(id: "k", label: "Keep"), .init(id: "d", label: "Delete")], multiple: false)))
        let option = try #require(chrome.optionItems.first)
        #expect(chrome.isOverChrome(option.frame.center))
        #expect(chrome.isOverChrome(chrome.buttons.mic))
        #expect(chrome.isOverChrome(chrome.buttons.down))
        if case .arc(let band)? = chrome.question?.background {
            let mid = band.point(angle: (band.startAngle + band.endAngle) / 2, radius: band.midRadius)
            #expect(chrome.isOverChrome(mid))
        }
        #expect(!chrome.isOverChrome(CGPoint(x: 1, y: 1)))
        #expect(!chrome.isOverChrome(CGPoint(x: F.layout(.bottom).panelSize.width - 2, y: 2)))
    }

    @Test("rotated rects and arc bands hit-test in their own frames")
    func shapes() {
        let rect = RotatedRect(center: CGPoint(x: 100, y: 100), size: CGSize(width: 40, height: 10), rotation: .pi / 2)
        #expect(rect.contains(CGPoint(x: 100, y: 118)))
        #expect(!rect.contains(CGPoint(x: 118, y: 100)))
        let band = ArcBand(center: .zero, innerRadius: 90, outerRadius: 110, startAngle: -0.3, endAngle: 0.3)
        #expect(band.contains(CGPoint(x: 100, y: 0)))
        #expect(!band.contains(CGPoint(x: 0, y: 100)))
        #expect(band.contains(band.point(angle: 0.3 + 0.05, radius: 100)), "round caps count")
        #expect(wrappedAngle(3 * .pi) == .pi)
        #expect(abs(wrappedAngle(-3 * .pi / 2) - .pi / 2) < 1e-12)
    }

    // MARK: Compact

    @Test("compact: the small visual on the left, content and buttons in the region right of it, no background")
    func compact() throws {
        for slot in SlotIndex.allCases {
            let slotLayout = F.layout(slot, .compact)
            let chrome = compute(slot, ChromeContent(badge: .simulation, elements: [.text("Now playing"),
                                                                                   .image(key: "c", aspect: 1, caption: "CO2")]),
                                 mode: .compact)
            let strip = try #require(chrome.stripRect)
            #expect(CGRect(origin: .zero, size: slotLayout.panelSize).contains(strip), "slot \(slot)")
            #expect(strip.minX >= slotLayout.visualRect.maxX - 1, "the region starts right of the visual, slot \(slot)")
            for item in chrome.items {
                #expect(strip.insetBy(dx: -1, dy: -1).contains(item.frame.boundingBox), "slot \(slot) \(item.kind)")
                #expect(item.frame.center.x > slotLayout.visualRect.maxX, "content is right of the visual")
                #expect(chrome.isOverChrome(item.frame.center))
            }
            for point in [chrome.buttons.mic, chrome.buttons.keyboard, chrome.buttons.down] {
                #expect(strip.contains(point))
                #expect(chrome.isOverChrome(point))
            }
            // ui-feedback.md #8: nothing is drawn between the elements, so clicks there pass through.
            let empty = CGPoint(x: strip.maxX - 40, y: strip.maxY - 3)
            #expect(!chrome.items.contains { $0.frame.contains(empty, slop: 2) })
            #expect(!chrome.isOverChrome(empty), "slot \(slot)")
        }
        let ask = compute(.top, ChromeContent(question: "Keep the file?", controls: .scale(isRange: true)), mode: .compact)
        #expect(ask.question != nil)
        if case .line? = ask.track {} else { Issue.record("compact tracks are straight") }
    }

    // MARK: Sliders

    @Test("stepped sliders get a dot per step when there are a sensible number of them")
    func stepDots() {
        #expect(ScaleTrackView.dottedSteps(spec: (min: 0, max: 10, step: 1, unit: nil), trackLength: 400) == 10)
        #expect(ScaleTrackView.dottedSteps(spec: (min: 0, max: 100, step: 5, unit: "%"), trackLength: 400) == 20)
        #expect(ScaleTrackView.dottedSteps(spec: (min: 6, max: 22, step: 1, unit: "h"), trackLength: 400) == 16)
        #expect(ScaleTrackView.dottedSteps(spec: (min: 0, max: 100, step: 1, unit: nil), trackLength: 400) == nil, "too many")
        #expect(ScaleTrackView.dottedSteps(spec: (min: 0, max: 20, step: 1, unit: nil), trackLength: 120) == nil, "too dense")
        #expect(ScaleTrackView.dottedSteps(spec: (min: 0, max: 1, step: 1, unit: nil), trackLength: 400) == nil, "one step")
        #expect(ScaleTrackView.dottedSteps(spec: (min: 0, max: 1, step: 0.3, unit: nil), trackLength: 400) == nil, "uneven")
    }

    // MARK: Values

    @Test("slider values use the step's decimals; currency leads, % attaches, other units follow")
    func valueFormatting() {
        #expect(SlotChromeModel.format(42, places: 0, unit: "%") == "42%")
        #expect(SlotChromeModel.format(120, places: 0, unit: "$") == "$120")
        #expect(SlotChromeModel.format(-5, places: 0, unit: "€") == "-€5")
        #expect(SlotChromeModel.format(3.5, places: 1, unit: "km") == "3.5 km")
        #expect(SlotChromeModel.format(7, places: 2, unit: nil) == "7.00")
    }

    // MARK: Shading

    @Test("pills are dark over light backdrops and light over dark ones")
    func shade() {
        #expect(PillShade.forBackdrop(.fromAppearance(.light)).tone == .dark)
        #expect(PillShade.forBackdrop(.fromAppearance(.dark)).tone == .light)
        let midGray = Backdrop.sample(red: 0.735, green: 0.735, blue: 0.735, source: .wallpaper, previousTone: .dark)
        #expect(PillShade.forBackdrop(midGray).tone == .light, "hysteresis keeps the dark tone near mid-grey")
    }
}

extension ChromeLayout.QuestionLine.Placement {
    /// Screen y of the line's middle at the arc's apex (or the block's centre).
    var arcApexY: CGFloat {
        switch self {
        case .arc(let arc, _): arc.apex.y
        case .block(let rect, _): rect.center.y
        }
    }
}
