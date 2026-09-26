// visual.md A8 example 6, adapted per BLUEPRINT §0.1: an eye that looks inward, and at the pointer when
// hovered. It blinks every ~4 s and on speech level peaks (there is no word timing).
let lx = 0, ly = 0

peek.frame((ctx, input) => {
  const a = input.hover ? input.mouse.angle : input.slot.facing
  const d = input.hover ? Math.min(10, input.mouse.dist / 4) : 6
  lx += (Math.cos(a) * d - lx) * Math.min(1, input.dt * 10)
  ly += (Math.sin(a) * d - ly) * Math.min(1, input.dt * 10)

  // blink every ~4s, and on loud moments of speech
  const blink = (input.t % 4) < 0.12 || (input.speech?.level ?? 0) > 0.6

  ctx.fillStyle = '#fff'
  ctx.beginPath(); ctx.ellipse(50, 50, 30, blink ? 2 : 22, 0, 0, Math.PI * 2); ctx.fill()

  if (!blink) {
    ctx.fillStyle = '#111'
    ctx.beginPath(); ctx.arc(50 + lx, 50 + ly, 9, 0, Math.PI * 2); ctx.fill()
  }

  return true   // always blinking
})
