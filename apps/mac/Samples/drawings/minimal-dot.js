// visual.md A8 example 1: a dot that pulses with any voice.
let s = 0

peek.frame((ctx, input) => {
  const voice = Math.max(input.speech?.level ?? 0, input.mic.level)
  s += (voice - s) * Math.min(1, input.dt * 12)

  ctx.fillStyle = input.backdrop.ink
  ctx.beginPath()
  ctx.arc(50, 50, 18 + s * 14, 0, Math.PI * 2)
  ctx.fill()

  return s > 0.001
})
