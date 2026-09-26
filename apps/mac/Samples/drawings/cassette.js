// visual.md A8 example 3, adapted per BLUEPRINT §0.1: clear glass with a light tint, a fixed 'SIDE A'
// label (there is no word timing), clipped tape, spinning reels and a bump on click.
const REELS = [[30, 50], [70, 50]]

// body with two reel holes, built once. Each hole starts its own subpath (moveTo): under canvas rules an
// arc() otherwise draws a line from the current point, which would cut a wedge out of the glass.
const body = new Path2D()
body.roundRect(5, 21, 90, 58, 7)
for (const [x, y] of REELS) { body.moveTo(x + 9, y); body.arc(x, y, 9, 0, Math.PI * 2) }

let spin = 0, bump = 0

peek.on('click', () => { bump = 1 })

// '#c8352b' → 'rgba(200, 53, 43, 0.3)': keeps the cover's hue but stays light, so the glass stays clear
function lightTint(hex) {
  const m = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(hex)
  if (!m) return 'rgba(232, 217, 184, 0.35)'
  return `rgba(${parseInt(m[1], 16)}, ${parseInt(m[2], 16)}, ${parseInt(m[3], 16)}, 0.3)`
}

peek.frame((ctx, input) => {
  const voice  = Math.max(input.speech?.level ?? 0, input.mic.level)
  const moving = input.phase === 'speaking' || input.phase === 'listening'
  spin += (moving ? 2 + voice * 8 : 0) * input.dt
  bump  = Math.max(0, bump - input.dt * 4)

  // tint from the cover art if one is being shown
  const art  = input.show?.elements.find(e => e.type === 'image')
  const tint = lightTint(art?.colors.dominant ?? '#e8d9b8')

  // 1. glass body (same outline every frame → never rebuilt; tint changes only per send)
  ctx.fillGlass(body, { style: 'clear', tint, interactive: true, rule: 'evenodd' })

  if (input.mode === 'compact') return moving || bump > 0   // tiny: glass only

  // 2. label
  ctx.fillStyle = 'rgba(255,255,255,0.6)'
  ctx.beginPath(); ctx.roundRect(13, 25, 74, 10, 2); ctx.fill()
  ctx.fillStyle = '#1c1c1c'
  ctx.font = '600 6px SF Pro'
  ctx.textAlign = 'center'
  ctx.fillText('SIDE A', 50, 32)

  // 3. tape window with moving stripes
  ctx.save()
  ctx.beginPath(); ctx.roundRect(40, 45, 20, 10, 3); ctx.clip()
  ctx.fillStyle = '#3a2a1a'
  for (let x = 34; x < 66; x += 5) ctx.fillRect(x + (spin * 4) % 5, 48, 2.5, 4)
  ctx.restore()

  // 4. reels
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
