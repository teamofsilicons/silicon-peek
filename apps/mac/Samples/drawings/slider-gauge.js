// visual.md A8 example 5: a gauge that follows a slider ask live.
let shown = 0

peek.frame((ctx, input) => {
  const ask = input.ask
  let target = 0
  if (ask?.type === 'slider' && typeof ask.value === 'number') {
    target = (ask.value - ask.min) / (ask.max - ask.min)
  }
  shown += (target - shown) * Math.min(1, input.dt * 14)

  const start = Math.PI * 0.75, sweep = Math.PI * 1.5

  ctx.lineCap = 'round'
  ctx.lineWidth = 8

  ctx.strokeStyle = input.appearance === 'dark' ? 'rgba(255,255,255,0.15)' : 'rgba(0,0,0,0.12)'
  ctx.beginPath(); ctx.arc(50, 50, 34, start, start + sweep); ctx.stroke()

  ctx.vibrant = true
  ctx.strokeStyle = input.backdrop.ink
  ctx.beginPath(); ctx.arc(50, 50, 34, start, start + sweep * shown); ctx.stroke()
  ctx.vibrant = false

  return Math.abs(target - shown) > 0.001
})
