// visual.md A8 example 2: a glass orb with a voice ring.
const orb = new Path2D()
orb.arc(50, 50, 40, 0, Math.PI * 2)

let lvl = 0

peek.frame((ctx, input) => {
  const voice = Math.max(input.speech?.level ?? 0, input.mic.level)
  lvl += (voice - lvl) * Math.min(1, input.dt * 10)

  // still glass body: created once, never rebuilt
  ctx.fillGlass(orb, { interactive: true })

  // ring on top of the glass, wobbling with voice
  ctx.strokeStyle = input.phase === 'listening' ? '#ff3b30' : input.backdrop.ink
  ctx.lineWidth = 2
  ctx.beginPath()
  for (let i = 0; i <= 64; i++) {
    const a = (i / 64) * Math.PI * 2
    const r = 30 + Math.sin(a * 6 + input.t * 5) * lvl * 6
    const x = 50 + Math.cos(a) * r, y = 50 + Math.sin(a) * r
    i ? ctx.lineTo(x, y) : ctx.moveTo(x, y)
  }
  ctx.closePath()
  ctx.stroke()

  return lvl > 0.001 || input.phase === 'listening'
})
