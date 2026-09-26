import CoreGraphics
import Foundation
import PeekCore
import Testing

@testable import PeekDrawing

/// Pixel checks of the CG replay (visual.md B6), rendered at 1 px per unit in sRGB.
@Suite("CG replay")
struct ReplayTests {
    @Test("fillRect paints its rectangle only, y-down")
    func fillRect() async throws {
        let image = try await renderScript("ctx.fillStyle = '#ff0000'; ctx.fillRect(10, 20, 30, 10)")
        #expect(pixel(image, x: 25, y: 25) == (255, 0, 0, 255))
        #expect(pixel(image, x: 25, y: 35).a == 0)
        #expect(pixel(image, x: 5, y: 25).a == 0)
        #expect(pixel(image, x: 25, y: 15).a == 0)
    }

    @Test("even-odd fills leave holes, nonzero fills do not")
    func fillRules() async throws {
        let evenOdd = try await renderScript("""
            ctx.fillStyle = 'blue'; ctx.beginPath(); ctx.arc(50, 50, 40, 0, 7); ctx.arc(50, 50, 20, 0, 7); ctx.fill('evenodd')
            """)
        #expect(pixel(evenOdd, x: 50, y: 50).a == 0)
        #expect(pixel(evenOdd, x: 50, y: 20) == (0, 0, 255, 255))
        let nonZero = try await renderScript("""
            ctx.fillStyle = 'blue'; ctx.beginPath(); ctx.arc(50, 50, 40, 0, 7); ctx.arc(50, 50, 20, 0, 7); ctx.fill()
            """)
        #expect(pixel(nonZero, x: 50, y: 50) == (0, 0, 255, 255))
    }

    @Test("transforms apply to points as they are added (canvas semantics)")
    func transforms() async throws {
        let image = try await renderScript("""
            ctx.fillStyle = 'lime'
            ctx.beginPath(); ctx.moveTo(0, 0); ctx.lineTo(10, 0); ctx.translate(50, 50); ctx.lineTo(10, 10); ctx.lineTo(0, 10)
            ctx.fill()
            ctx.setTransform(1, 0, 0, 1, 0, 0)
            ctx.translate(80, 80); ctx.rotate(Math.PI / 4); ctx.fillRect(-5, -5, 10, 10)
            """)
        // Quad (0,0) (10,0) (60,60) (50,60): its interior at (30, 32) is filled.
        #expect(pixel(image, x: 30, y: 32).g == 255)
        // The rotated square is a diamond: its corner points lie on the axes.
        #expect(pixel(image, x: 80, y: 76).a == 255)
        #expect(pixel(image, x: 75, y: 75).a == 0)
    }

    @Test("clip limits painting and restore removes it")
    func clip() async throws {
        let image = try await renderScript("""
            ctx.save(); ctx.beginPath(); ctx.rect(0, 0, 50, 100); ctx.clip()
            ctx.fillStyle = 'red'; ctx.fillRect(0, 0, 100, 50)
            ctx.restore()
            ctx.fillStyle = 'blue'; ctx.fillRect(0, 60, 100, 40)
            """)
        #expect(pixel(image, x: 25, y: 25) == (255, 0, 0, 255))
        #expect(pixel(image, x: 75, y: 25).a == 0)
        #expect(pixel(image, x: 75, y: 80) == (0, 0, 255, 255))
    }

    @Test("globalAlpha, destination-out and clearRect composite like canvas")
    func compositing() async throws {
        let image = try await renderScript("""
            ctx.globalAlpha = 0.5; ctx.fillStyle = 'black'; ctx.fillRect(0, 0, 50, 50)
            ctx.globalAlpha = 1; ctx.fillStyle = 'white'; ctx.fillRect(50, 0, 50, 50)
            ctx.globalCompositeOperation = 'destination-out'; ctx.fillRect(60, 10, 10, 10)
            ctx.globalCompositeOperation = 'source-over'
            ctx.fillStyle = 'red'; ctx.fillRect(0, 50, 100, 50); ctx.clearRect(10, 60, 10, 10)
            """)
        #expect(abs(pixel(image, x: 25, y: 25).a - 128) <= 2)
        #expect(pixel(image, x: 65, y: 15).a == 0)
        #expect(pixel(image, x: 85, y: 15) == (255, 255, 255, 255))
        #expect(pixel(image, x: 15, y: 65).a == 0)
        #expect(pixel(image, x: 30, y: 65) == (255, 0, 0, 255))
    }

    @Test("strokes use the transform at stroke time for their width")
    func strokeWidth() async throws {
        let image = try await renderScript("""
            ctx.strokeStyle = 'black'; ctx.lineWidth = 4
            ctx.beginPath(); ctx.moveTo(10, 20); ctx.lineTo(90, 20)
            ctx.scale(1, 3); ctx.stroke()
            """)
        // 4 units wide × 3 = 12 units tall around y = 20.
        #expect(pixel(image, x: 50, y: 15).a == 255)
        #expect(pixel(image, x: 50, y: 25).a == 255)
        #expect(pixel(image, x: 50, y: 28).a == 0)
    }

    @Test("line dashes leave gaps")
    func dashes() async throws {
        let image = try await renderScript("""
            ctx.strokeStyle = 'black'; ctx.lineWidth = 2; ctx.setLineDash([10, 10])
            ctx.beginPath(); ctx.moveTo(0, 50); ctx.lineTo(100, 50); ctx.stroke()
            """)
        #expect(pixel(image, x: 5, y: 50).a == 255)
        #expect(pixel(image, x: 15, y: 50).a == 0)
        #expect(pixel(image, x: 25, y: 50).a == 255)
    }

    @Test("linear and radial gradients follow canvas geometry")
    func gradients() async throws {
        let linear = try await renderScript("""
            const g = ctx.createLinearGradient(0, 0, 100, 0); g.addColorStop(0, 'red'); g.addColorStop(1, 'blue')
            ctx.fillStyle = g; ctx.fillRect(0, 0, 100, 100)
            """)
        let left = pixel(linear, x: 2, y: 50), right = pixel(linear, x: 97, y: 50)
        #expect(left.r > 240 && left.b < 20)
        #expect(right.b > 240 && right.r < 20)
        let radial = try await renderScript("""
            const g = ctx.createRadialGradient(50, 50, 0, 50, 50, 50); g.addColorStop(0, 'white'); g.addColorStop(1, 'black')
            ctx.fillStyle = g; ctx.fillRect(0, 0, 100, 100)
            """)
        #expect(pixel(radial, x: 50, y: 50).r > 240)
        #expect(pixel(radial, x: 2, y: 2).r < 10)
    }

    @Test("conic gradients start at startAngle and run clockwise on screen")
    func conicGradient() async throws {
        let image = try await renderScript("""
            const g = ctx.createConicGradient(0, 50, 50)
            g.addColorStop(0, 'red'); g.addColorStop(0.25, 'lime'); g.addColorStop(0.5, 'blue')
            g.addColorStop(0.75, 'yellow'); g.addColorStop(1, 'red')
            ctx.fillStyle = g; ctx.fillRect(0, 0, 100, 100)
            """)
        let right = pixel(image, x: 95, y: 52), below = pixel(image, x: 50, y: 95)
        let left = pixel(image, x: 5, y: 50), above = pixel(image, x: 50, y: 5)
        #expect(right.r > 200 && right.g < 80, "right \(right)")
        #expect(below.g > 200 && below.r < 80 && below.b < 80, "below \(below)")
        #expect(left.b > 200 && left.r < 80, "left \(left)")
        #expect(above.r > 200 && above.g > 200 && above.b < 80, "above \(above)")
    }

    @Test("text is drawn upright with Core Text at the requested alignment and baseline")
    func text() async throws {
        let image = try await renderScript("""
            ctx.fillStyle = 'black'; ctx.font = 'bold 20px SF Pro'; ctx.textAlign = 'center'; ctx.textBaseline = 'middle'
            ctx.fillText('HH', 50, 50)
            """)
        var inked: [(Int, Int)] = []
        for y in stride(from: 0, to: 100, by: 1) {
            for x in stride(from: 0, to: 100, by: 1) where pixel(image, x: x, y: y).a > 128 {
                inked.append((x, y))
            }
        }
        let xs = inked.map(\.0), ys = inked.map(\.1)
        let midX = Double(xs.min()! + xs.max()!) / 2, midY = Double(ys.min()! + ys.max()!) / 2
        #expect(abs(midX - 50) < 3, "x \(xs.min()!)…\(xs.max()!)")
        #expect(abs(midY - 50) < 4, "y \(ys.min()!)…\(ys.max()!)")
        // Cap height of 20 px bold is ≈ 14 px: upright glyphs, not mirrored or squashed.
        #expect((10...18).contains(ys.max()! - ys.min()!))
    }

    @Test("drawImage draws a live handle into its destination, upright")
    func drawImage() async throws {
        let top = solidImage(width: 10, height: 5, red: 1, green: 0, blue: 0)
        let bottom = solidImage(width: 10, height: 5, red: 0, green: 0, blue: 1)
        let context = CGContext(data: nil, width: 10, height: 10, bitsPerComponent: 8, bytesPerRow: 0,
                                space: CGColorSpace(name: CGColorSpace.sRGB)!,
                                bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
        context.draw(bottom, in: CGRect(x: 0, y: 0, width: 10, height: 5))
        context.draw(top, in: CGRect(x: 0, y: 5, width: 10, height: 5))
        let split = context.makeImage()!
        let image = try await renderScript("ctx.drawImage({ id: 4, width: 10, height: 10 }, 20, 20, 40, 40)",
                                           images: [4: split])
        #expect(pixel(image, x: 40, y: 25) == (255, 0, 0, 255))  // the image's top half stays on top
        #expect(pixel(image, x: 40, y: 55) == (0, 0, 255, 255))
        #expect(pixel(image, x: 10, y: 10).a == 0)
        // An unknown handle draws nothing.
        let none = try await renderScript("ctx.drawImage({ id: 9, width: 10, height: 10 }, 0, 0, 100, 100)")
        #expect(pixel(none, x: 50, y: 50).a == 0)
    }

    @Test("shadows are offset in units, unaffected by the transform")
    func shadow() async throws {
        let image = try await renderScript("""
            ctx.shadowColor = 'rgba(0,0,255,1)'; ctx.shadowOffsetX = 20; ctx.shadowOffsetY = 10
            ctx.scale(2, 2); ctx.fillStyle = 'red'; ctx.fillRect(5, 5, 10, 10)
            """)
        #expect(pixel(image, x: 15, y: 15) == (255, 0, 0, 255))
        let shadowPixel = pixel(image, x: 45, y: 35)
        #expect(shadowPixel.b > 200 && shadowPixel.r < 30, "\(shadowPixel)")
    }

    @Test("filter blur softens edges")
    func blurFilter() async throws {
        let sharp = try await renderScript("ctx.fillStyle = 'black'; ctx.fillRect(30, 30, 40, 40)")
        let blurred = try await renderScript("ctx.filter = 'blur(4px)'; ctx.fillStyle = 'black'; ctx.fillRect(30, 30, 40, 40)")
        #expect(pixel(sharp, x: 27, y: 50).a == 0)
        let edge = pixel(blurred, x: 27, y: 50).a
        #expect(edge > 20 && edge < 200, "\(edge)")
        #expect(pixel(blurred, x: 50, y: 50).a > 240)
    }

    @Test("vibrant layers render in grey")
    func vibrantMonochrome() async throws {
        let image = try await renderScript("ctx.vibrant = true; ctx.fillStyle = '#ff0000'; ctx.fillRect(0, 0, 100, 100)")
        let p = pixel(image, x: 50, y: 50)
        #expect(p.r == p.g && p.g == p.b, "\(p)")
    }

    @Test("renders into Display P3 bitmaps by default")
    func displayP3() throws {
        let decoder = OpDecoder()
        let list = decoder.decode(ops: [Double(OpCode.fillRect.rawValue), 4, 0, 0, 100, 100], strings: [], again: false)
        guard case .draw(let draw) = list.layers[0] else {
            Issue.record("expected a draw layer")
            return
        }
        let image = try #require(CGReplay.render(draw.commands, pixels: 240, environment: ReplayEnvironment()))
        #expect(image.width == 240)
        #expect(image.colorSpace?.name == CGColorSpace.displayP3)
        #expect(pixel(image, x: 120, y: 120) == (0, 0, 0, 255))
    }
}
