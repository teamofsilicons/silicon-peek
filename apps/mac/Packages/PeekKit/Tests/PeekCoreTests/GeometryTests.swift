import CoreGraphics
import Foundation
import Testing

@testable import PeekCore

@Suite("Slot geometry")
struct GeometryTests {
    /// A 14" MacBook screen below the menu bar, in AppKit coordinates (y up).
    static let laptop = CGRect(x: 0, y: 0, width: 1512, height: 944)
    /// A secondary display left of and above the primary one.
    static let external = CGRect(x: -2560, y: 200, width: 2560, height: 1415)

    /// Screen point → y-down coordinates relative to the visible frame's top-left.
    private func imagePoint(_ p: CGPoint, in vf: CGRect) -> CGPoint { CGPoint(x: p.x - vf.minX, y: vf.maxY - p.y) }

    private func distance(_ a: CGPoint, _ b: CGPoint) -> CGFloat { hypot(a.x - b.x, a.y - b.y) }

    @Test("the arc matches peek all-positions.jpg (1280 × 832) within 20 px (the image is hand-drawn)")
    func matchesReferenceImage() throws {
        let vf = CGRect(x: 0, y: 0, width: 1280, height: 832)
        // Measured from the image: circle centres and arc endpoints (y-down).
        let expected: [(SlotIndex, circle: CGPoint, ends: (CGPoint, CGPoint))] = [
            (.top, CGPoint(x: 640, y: 108), (CGPoint(x: 460, y: 153), CGPoint(x: 820, y: 153))),
            (.left, CGPoint(x: 110, y: 415), (CGPoint(x: 155, y: 238), CGPoint(x: 157, y: 593))),
            (.topLeft, CGPoint(x: 115, y: 118), (CGPoint(x: 20, y: 280), CGPoint(x: 275, y: 28))),
            (.bottom, CGPoint(x: 640, y: 733), (CGPoint(x: 460, y: 688), CGPoint(x: 820, y: 688))),
        ]
        for (slot, circle, ends) in expected {
            let layout = SlotGeometry.layout(slot: slot, mode: .normal, visibleFrame: vf)
            let arc = try #require(layout.arc)
            let centre = imagePoint(layout.screenPoint(fromPanel: layout.visualCenter), in: vf)
            #expect(distance(centre, circle) < 20, "slot \(slot) circle at \(centre), image has \(circle)")
            let a = imagePoint(layout.screenPoint(fromPanel: arc.point(atArcLength: -arc.length / 2)), in: vf)
            let b = imagePoint(layout.screenPoint(fromPanel: arc.point(atArcLength: arc.length / 2)), in: vf)
            let matched = (distance(a, ends.0) < 20 && distance(b, ends.1) < 20) || (distance(a, ends.1) < 20 && distance(b, ends.0) < 20)
            #expect(matched, "slot \(slot) arc ends \(a) \(b), image has \(ends.0) \(ends.1)")
        }
    }

    @Test("proportions: Ø ≈ 12% of width, apex 1.3 r, radius 4.5 r, endpoints ≈ 2.45 r, span ±32°")
    func proportions() throws {
        let layout = SlotGeometry.layout(slot: .top, mode: .normal, visibleFrame: Self.laptop)
        let arc = try #require(layout.arc)
        let r = layout.visualRadius
        #expect(abs(layout.visualRect.width - 1512 * 0.12) < 0.01)
        #expect(abs(distance(arc.apex, layout.visualCenter) - 1.3 * r) < 0.01)
        #expect(abs(arc.radius - 4.5 * r) < 0.01)
        #expect(abs(arc.halfSpan - 32 * .pi / 180) < 1e-9)
        let endDistance = distance(arc.point(atArcLength: arc.length / 2), layout.visualCenter) / r
        #expect(endDistance > 2.3 && endDistance < 2.6)
        #expect(abs(layout.facing - .pi / 2) < 1e-9)
        #expect(layout.visualRect.width == SlotGeometry.visualDiameter(screenWidth: 1512))
        #expect(SlotGeometry.visualDiameter(screenWidth: 400) == 88)
        #expect(SlotGeometry.visualDiameter(screenWidth: 6000) == 240)
    }

    @Test("every panel lies inside the visible frame and touches the edge it slides out of", arguments: [laptop, external])
    func panelsInsideAndTouchingEdges(vf: CGRect) {
        for mode in DisplayMode.allCases {
            for slot in SlotIndex.allCases {
                let layout = SlotGeometry.layout(slot: slot, mode: mode, visibleFrame: vf)
                let panel = layout.panelFrame
                #expect(vf.contains(panel), "\(mode) \(slot): \(panel) outside \(vf)")
                let side = slot.side
                if side.inward.dy > 0.1 { #expect(panel.maxY == vf.maxY, "\(mode) \(slot) must touch the top edge") }
                if side.inward.dy < -0.1 { #expect(panel.minY == vf.minY, "\(mode) \(slot) must touch the bottom edge") }
                if side.inward.dx > 0.1 { #expect(panel.minX == vf.minX, "\(mode) \(slot) must touch the left edge") }
                if side.inward.dx < -0.1 { #expect(panel.maxX == vf.maxX, "\(mode) \(slot) must touch the right edge") }
                // Whole points only, so windows land on pixel boundaries.
                #expect(panel.minX == panel.minX.rounded() && panel.width == panel.width.rounded())
            }
        }
    }

    @Test("the visual and the whole arc band fit inside the panel", arguments: [laptop, external])
    func contentInsidePanel(vf: CGRect) throws {
        for slot in SlotIndex.allCases {
            let layout = SlotGeometry.layout(slot: slot, mode: .normal, visibleFrame: vf)
            let bounds = CGRect(origin: .zero, size: layout.panelSize)
            #expect(bounds.contains(layout.visualRect), "\(slot) visual outside the panel")
            let arc = try #require(layout.arc)
            for s in stride(from: -arc.length / 2, through: arc.length / 2, by: arc.length / 16) {
                for offset in [0, arc.bandDepth] {
                    let p = arc.point(atArcLength: s, offset: offset)
                    #expect(bounds.insetBy(dx: -0.5, dy: -0.5).contains(p), "\(slot) arc point \(p) outside \(bounds)")
                }
            }
            #expect(bounds.contains(layout.contentBounds.insetBy(dx: 0.5, dy: 0.5)))
        }
    }

    @Test("the hidden offset moves all content beyond the panel through the outward edge")
    func hiddenOffset() {
        for mode in DisplayMode.allCases {
            for slot in SlotIndex.allCases {
                let layout = SlotGeometry.layout(slot: slot, mode: mode, visibleFrame: Self.laptop)
                let moved = layout.contentBounds.offsetBy(dx: layout.hiddenOffset.dx, dy: layout.hiddenOffset.dy)
                let panel = CGRect(origin: .zero, size: layout.panelSize)
                #expect(!moved.intersects(panel), "\(mode) \(slot): hidden content \(moved) still overlaps \(panel)")
                // Direction is outward: opposite the inward compass vector (y-down).
                let inward = slot.side.inward
                #expect(layout.hiddenOffset.dx * inward.dx + layout.hiddenOffset.dy * inward.dy < 0)
            }
        }
    }

    @Test("arcs face the screen centre in compass steps; facing points at the true centre")
    func facing() throws {
        let topLeft = SlotGeometry.layout(slot: .topLeft, mode: .normal, visibleFrame: Self.laptop)
        #expect(abs(try #require(topLeft.arc).normalAngle - .pi / 4) < 1e-9)
        let centre = topLeft.panelPoint(fromScreen: CGPoint(x: Self.laptop.midX, y: Self.laptop.midY))
        let expected = atan2(Double(centre.y - topLeft.visualCenter.y), Double(centre.x - topLeft.visualCenter.x))
        #expect(abs(topLeft.facing - expected) < 1e-9)
        #expect(topLeft.facing < .pi / 4)  // a 16:10 screen is wider than tall

        let right = SlotGeometry.layout(slot: .right, mode: .normal, visibleFrame: Self.laptop)
        #expect(abs(abs(right.facing) - .pi) < 1e-9)
        let bottom = SlotGeometry.layout(slot: .bottom, mode: .normal, visibleFrame: Self.laptop)
        #expect(abs(bottom.facing + .pi / 2) < 1e-9)
    }

    @Test("three elements are flex-centred, each at most a third of the arc, in reading order")
    func placement() throws {
        let layout = SlotGeometry.layout(slot: .bottom, mode: .normal, visibleFrame: Self.laptop)
        let arc = try #require(layout.arc)
        let side = arc.maxElementAlongExtent
        #expect(abs(side - (arc.length - 2 * arc.gap) / 3) < 1e-9)
        let placements = layout.placeElements([
            ElementRequest(size: CGSize(width: 60, height: 24), fit: .truncate),
            ElementRequest(size: CGSize(width: 1000, height: 1000), fit: .scale),
            ElementRequest(size: CGSize(width: 60, height: 24), fit: .truncate),
        ])
        #expect(placements.count == 3)
        #expect(abs(placements[1].size.width - side) < 1e-6)
        #expect(abs(placements[1].size.height - side) < 1e-6)
        #expect(abs(placements[1].arcLength) < 1e-6)  // centred on the apex
        #expect(abs(placements[1].rotation) < 1e-9)
        #expect(abs(placements[0].arcLength + placements[2].arcLength) < 1e-6)  // symmetric
        #expect(placements[0].center.x < placements[1].center.x && placements[1].center.x < placements[2].center.x)
        // Bottom slot: content sits above the baseline (toward the screen centre).
        let baseline = arc.point(atArcLength: 0)
        #expect(placements[1].center.y < baseline.y)
        #expect(abs((baseline.y - placements[1].center.y) - side / 2) < 1e-6)
        // The left pill tilts counter-clockwise, like the "2022" pill in peek in-pactice.jpg.
        #expect(placements[0].rotation < 0)
        #expect(placements[2].rotation > 0)
    }

    @Test("top slot content hangs below the baseline and stays upright")
    func topPlacement() throws {
        let layout = SlotGeometry.layout(slot: .top, mode: .normal, visibleFrame: Self.laptop)
        let arc = try #require(layout.arc)
        let placement = try #require(layout.placeElements([ElementRequest(size: CGSize(width: 80, height: 30), fit: .truncate)]).first)
        #expect(placement.center.y > arc.apex.y)
        #expect(abs(placement.rotation) < 1e-9)
        let two = layout.placeElements([ElementRequest(size: CGSize(width: 80, height: 30), fit: .truncate),
                                        ElementRequest(size: CGSize(width: 80, height: 30), fit: .truncate)])
        #expect(two[0].center.x < two[1].center.x)
        #expect(two[1].rotation < 0)  // right end of a smile rises
    }

    @Test("side slots stack upright elements top to bottom; the along extent is their height")
    func sidePlacement() throws {
        let layout = SlotGeometry.layout(slot: .left, mode: .normal, visibleFrame: Self.laptop)
        let arc = try #require(layout.arc)
        let placements = layout.placeElements([
            ElementRequest(size: CGSize(width: 120, height: 40), fit: .truncate),
            ElementRequest(size: CGSize(width: 120, height: 40), fit: .truncate),
        ])
        #expect(placements[0].center.y < placements[1].center.y)
        #expect(abs(placements[0].rotation) < 0.2)
        // Content is right of the baseline (toward the screen centre), lifted by half its width.
        let base = arc.point(atArcLength: placements[0].arcLength)
        #expect(placements[0].center.x > base.x)
        #expect(abs(abs(placements[0].arcLength - placements[1].arcLength) - (40 + arc.gap)) < 1e-6)
        let tall = layout.placeElements([ElementRequest(size: CGSize(width: 50, height: 5000), fit: .truncate)])
        #expect(abs(tall[0].size.height - arc.maxElementAlongExtent) < 1e-6)
        #expect(tall[0].size.width == 50)
    }

    @Test("drawing units map the visual square to 0…100 from screen points")
    func drawingUnits() {
        let layout = SlotGeometry.layout(slot: .bottomRight, mode: .normal, visibleFrame: Self.external)
        let square = layout.visualFrameOnScreen
        let centre = layout.drawingUnits(fromScreen: CGPoint(x: square.midX, y: square.midY))
        #expect(abs(centre.x - 50) < 1e-9 && abs(centre.y - 50) < 1e-9)
        let topLeft = layout.drawingUnits(fromScreen: CGPoint(x: square.minX, y: square.maxY))
        #expect(abs(topLeft.x) < 1e-9 && abs(topLeft.y) < 1e-9)
        let bottomRight = layout.drawingUnits(fromScreen: CGPoint(x: square.maxX, y: square.minY))
        #expect(abs(bottomRight.x - 100) < 1e-9 && abs(bottomRight.y - 100) < 1e-9)
        let p = CGPoint(x: 31, y: 17)
        #expect(layout.panelPoint(fromScreen: layout.screenPoint(fromPanel: p)) == p)
    }

    @Test("compact mode: a small visual on the left and a straight line to its right, on every slot")
    func compact() throws {
        for slot in SlotIndex.allCases {
            let layout = SlotGeometry.layout(slot: slot, mode: .compact, visibleFrame: Self.laptop)
            #expect(layout.arc == nil)
            let line = try #require(layout.line)
            #expect(layout.visualRect.width <= 48 && layout.visualRect.width >= 32)
            #expect(line.start.x > layout.visualRect.maxX)
            #expect(abs(line.start.y - layout.visualCenter.y) < 1e-9)
            #expect(line.length >= 300 && line.length <= 460)
            let placements = layout.placeElements([
                ElementRequest(size: CGSize(width: 400, height: 40), fit: .truncate),
                ElementRequest(size: CGSize(width: 48, height: 48), fit: .scale),
            ])
            #expect(placements[0].size.width <= line.maxElementAlongExtent + 1e-6)
            #expect(placements.allSatisfy { $0.rotation == 0 })
            #expect(placements[0].center.x < placements[1].center.x)
        }
        let right = SlotGeometry.layout(slot: .right, mode: .compact, visibleFrame: Self.laptop)
        let screenLineEnd = right.screenPoint(fromPanel: try #require(right.line).end)
        #expect(abs(screenLineEnd.x - (Self.laptop.maxX - SlotGeometry.compactEdgeMargin)) < 1)
    }

    @Test("the question arc is concentric and outside the baseline")
    func questionArc() throws {
        let arc = try #require(SlotGeometry.layout(slot: .bottom, mode: .normal, visibleFrame: Self.laptop).arc)
        let question = arc.concentric(offset: 60)
        #expect(question.center == arc.center)
        #expect(question.radius == arc.radius + 60)
        #expect(distance(question.apex, arc.apex) - 60 < 1e-6)
    }
}
