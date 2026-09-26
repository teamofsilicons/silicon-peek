import CryptoKit
import Foundation

/// The sample drawing Simulation loads: visual.md Example 3 (the cassette), as amended by
/// BLUEPRINT §0.1 (no `speech.word`; `style:'clear'` glass with a light tint), plus:
///
/// * `peek.log` calls on every event, so the Simulation window has `peek.log` output to show;
/// * the label reads `SIMULATION` while `input.context` is `'simulation'` (else `SIDE A`), so a
///   simulated bubble is labelled even inside the drawing.
public enum SimulationCassette {
    public static let filename = "cassette.js"

    public static let source = #"""
        // cassette.js: the visual.md cassette (Example 3, BLUEPRINT §0.1), bundled with Peek's Simulation.
        const REELS = [[30, 50], [70, 50]]

        // Body with two reel holes, built once: the glass outline never changes, so it is never rebuilt.
        // Each hole starts its own subpath (moveTo): under canvas rules an arc() otherwise draws a line from
        // the current point, which would cut a wedge out of the glass.
        const body = new Path2D()
        body.roundRect(5, 21, 90, 58, 7)
        for (const [x, y] of REELS) { body.moveTo(x + 9, y); body.arc(x, y, 9, 0, Math.PI * 2) }

        // '#c8352b' → 'rgba(200, 53, 43, 0.3)': keeps the cover's hue but stays light, so the glass stays clear.
        function lightTint(hex) {
          const m = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(hex)
          if (!m) return 'rgba(232, 217, 184, 0.35)'
          return `rgba(${parseInt(m[1], 16)}, ${parseInt(m[2], 16)}, ${parseInt(m[3], 16)}, 0.3)`
        }

        let spin = 0, bump = 0

        peek.on('enter', () => peek.log('enter'))
        peek.on('leave', () => peek.log('leave'))
        peek.on('send', p => {
          const what = p.ask ? 'ask ' + p.ask.type
            : p.show ? 'show with ' + p.show.elements.length + ' element(s)'
            : 'speech only'
          peek.log('send:', what + (p.speech ? ', speaking' : ''))
        })
        peek.on('click', e => {
          bump = 1
          peek.log('click at', e.x.toFixed(1), e.y.toFixed(1), 'count', e.count)
        })
        peek.on('answer', a => peek.log('answer', JSON.stringify(a.value), 'via', a.via))
        peek.on('move', m => peek.log('move', m.from, '->', m.to))

        peek.frame((ctx, input) => {
          const voice  = Math.max(input.speech?.level ?? 0, input.mic.level)
          const moving = input.phase === 'speaking' || input.phase === 'listening'
          spin += (moving ? 2 + voice * 8 : 0) * input.dt
          bump  = Math.max(0, bump - input.dt * 4)

          // Tint from the cover art when one is shown.
          const art  = input.show?.elements.find(e => e.type === 'image')
          const tint = lightTint(art?.colors.dominant ?? '#e8d9b8')

          // 1. Glass body: same outline every frame; the tint changes only per send.
          ctx.fillGlass(body, { style: 'clear', tint, interactive: true, rule: 'evenodd' })

          if (input.mode === 'compact') return moving || bump > 0   // tiny: glass only

          // 2. Label
          ctx.fillStyle = 'rgba(255,255,255,0.6)'
          ctx.beginPath(); ctx.roundRect(13, 25, 74, 10, 2); ctx.fill()
          ctx.fillStyle = '#1c1c1c'
          ctx.font = '600 6px SF Pro'
          ctx.textAlign = 'center'
          ctx.fillText(input.context === 'simulation' ? 'SIMULATION' : 'SIDE A', 50, 32)

          // 3. Tape window with moving stripes
          ctx.save()
          ctx.beginPath(); ctx.roundRect(40, 45, 20, 10, 3); ctx.clip()
          ctx.fillStyle = '#3a2a1a'
          for (let x = 34; x < 66; x += 5) ctx.fillRect(x + (spin * 4) % 5, 48, 2.5, 4)
          ctx.restore()

          // 4. Reels
          for (const [x, y] of REELS) {
            ctx.save()
            ctx.translate(x, y)
            ctx.rotate(spin)
            ctx.scale(1 + bump * 0.15, 1 + bump * 0.15)
            ctx.strokeStyle = '#1c1c1c'; ctx.lineWidth = 1.6; ctx.lineCap = 'round'
            ctx.beginPath(); ctx.arc(0, 0, 3, 0, Math.PI * 2); ctx.stroke()
            for (let i = 0; i < 6; i++) {
              ctx.rotate(Math.PI / 3)
              ctx.beginPath(); ctx.moveTo(0, 3); ctx.lineTo(0, 7.5); ctx.stroke()
            }
            ctx.restore()
          }

          return moving || bump > 0
        })

        """#

    public static var data: Data { Data(source.utf8) }

    /// Lowercase hex SHA-256 of ``data`` (the `sha256` of `drawing.load`).
    public static var sha256: String {
        SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
    }
}
