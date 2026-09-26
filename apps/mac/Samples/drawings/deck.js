// Deck: the sample drawing Simulation uses (BLUEPRINT §8.11). A record spins behind a clear glass
// cassette shell, so the glass refracts it (the stacking rule in visual.md A5). The record label shows the
// cover art, the shell takes a light tint from it, the reels spin while peek speaks or listens, the tape
// window's playhead follows a slider or range ask, a VU meter shows the voice level, a red light blinks
// while listening, hovering speeds the reels up and a click bumps them. Compact mode keeps only the shell
// and the reels.

const SHELL = new Path2D()
SHELL.roundRect(6, 24, 88, 54, 8)
const REELS = [[31, 51], [69, 51]]
for (const [x, y] of REELS) { SHELL.moveTo(x + 8.5, y); SHELL.arc(x, y, 8.5, 0, Math.PI * 2) }
const SCREWS = [[11, 29], [89, 29], [11, 73], [89, 73]]

let spin = 0, disc = 0, level = 0, bump = 0, glow = 0, blink = 0

peek.on('click', () => { bump = 1 })
peek.on('send', payload => { if (payload && payload.show) disc += 0.6 })

// '#c8352b', 0.3 → 'rgba(200, 53, 43, 0.3)'
function rgba(hex, alpha) {
  const m = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(hex)
  if (!m) return `rgba(232, 217, 184, ${alpha})`
  return `rgba(${parseInt(m[1], 16)}, ${parseInt(m[2], 16)}, ${parseInt(m[3], 16)}, ${alpha})`
}

// Shortens text with an ellipsis until it fits (measureText results are cached by the prelude).
function fit(ctx, text, maxWidth) {
  if (ctx.measureText(text).width <= maxWidth) return text
  let lo = 0, hi = text.length
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1
    if (ctx.measureText(text.slice(0, mid) + '…').width <= maxWidth) lo = mid
    else hi = mid - 1
  }
  return text.slice(0, lo) + '…'
}

function reel(ctx, x, y, scale) {
  ctx.save()
  ctx.translate(x, y)
  ctx.rotate(spin)
  ctx.scale(scale, scale)
  ctx.strokeStyle = '#1c1c1c'
  ctx.lineWidth = 1.5
  ctx.lineCap = 'round'
  ctx.beginPath(); ctx.arc(0, 0, 2.8, 0, Math.PI * 2); ctx.stroke()
  for (let i = 0; i < 6; i++) {
    ctx.rotate(Math.PI / 3)
    ctx.beginPath(); ctx.moveTo(0, 2.8); ctx.lineTo(0, 7); ctx.stroke()
  }
  ctx.restore()
}

peek.frame((ctx, input) => {
  const voice = Math.max(input.speech?.level ?? 0, input.mic.level)
  level += (voice - level) * Math.min(1, input.dt * 12)
  const moving = input.phase === 'speaking' || input.phase === 'listening'
  const showing = input.phase === 'showing'
  spin += (moving ? 2 + level * 7 : input.hover ? 0.8 : 0) * input.dt
  disc += (moving || showing ? 0.9 : 0) * input.dt
  bump = Math.max(0, bump - input.dt * 4)
  const glowTarget = input.hover ? 1 : 0
  glow += (glowTarget - glow) * Math.min(1, input.dt * 8)
  blink = input.phase === 'listening' ? (blink + input.dt) % 1 : 0

  const art = input.show?.elements.find(e => e.type === 'image')
  const title = input.show?.elements.find(e => e.type === 'text')?.text
  const compact = input.mode === 'compact'

  // 1. the record, drawn before the glass so the glass refracts it
  if (!compact) {
    ctx.save()
    ctx.translate(50, 44)
    ctx.rotate(disc)
    ctx.fillStyle = '#141414'
    ctx.beginPath(); ctx.arc(0, 0, 30, 0, Math.PI * 2); ctx.fill()
    ctx.strokeStyle = 'rgba(255,255,255,0.08)'
    ctx.lineWidth = 0.5
    for (let r = 14; r < 30; r += 2.5) { ctx.beginPath(); ctx.arc(0, 0, r, 0, Math.PI * 2); ctx.stroke() }
    ctx.save()
    ctx.beginPath(); ctx.arc(0, 0, 11, 0, Math.PI * 2); ctx.clip()
    if (art) ctx.drawImage(art.image, -11, -11, 22, 22)
    else { ctx.fillStyle = '#c8352b'; ctx.fillRect(-11, -11, 22, 22) }
    ctx.restore()
    ctx.restore()
  }

  // 2. the glass shell: a fixed outline, so it is never rebuilt (the tint only changes per send)
  const tint = art ? rgba(art.colors.dominant, 0.28) : 'rgba(232, 217, 184, 0.3)'
  ctx.fillGlass(SHELL, { style: 'clear', tint, interactive: true, rule: 'evenodd' })

  // frosted glass has no rim highlight of its own (BLUEPRINT §0.1 item 5)
  if (input.glass === 'frosted') {
    ctx.strokeStyle = 'rgba(255,255,255,0.55)'
    ctx.lineWidth = 0.8
    ctx.stroke(SHELL)
  }

  // 3. reels, on top of the glass
  for (const [x, y] of REELS) reel(ctx, x, y, 1 + bump * 0.15)

  if (compact) return moving || bump > 0   // tiny: shell and reels only

  // 4. label strip with the show's text
  const paper = ctx.createLinearGradient(14, 0, 86, 0)
  paper.addColorStop(0, 'rgba(255,255,255,0.8)')
  paper.addColorStop(1, 'rgba(255,241,222,0.8)')
  ctx.fillStyle = paper
  ctx.beginPath(); ctx.roundRect(14, 28, 72, 9, 2); ctx.fill()
  ctx.fillStyle = '#1c1c1c'
  ctx.font = '600 5.5px SF Pro Rounded'
  ctx.textAlign = 'center'
  ctx.textBaseline = 'middle'
  ctx.fillText(fit(ctx, title ?? 'SIDE A', 66), 50, 32.6)

  // 5. tape window: stripes move with the reels; a slider or range ask moves the playhead
  ctx.save()
  ctx.beginPath(); ctx.roundRect(40, 46, 20, 10, 3); ctx.clip()
  ctx.fillStyle = '#3a2a1a'
  for (let x = 34; x < 66; x += 5) ctx.fillRect(x + (spin * 4) % 5, 49, 2.5, 4)
  const ask = input.ask
  let head = null
  if (ask?.type === 'slider' && typeof ask.value === 'number') head = (ask.value - ask.min) / (ask.max - ask.min)
  else if (ask?.type === 'range' && Array.isArray(ask.value)) head = (ask.value[1] - ask.min) / (ask.max - ask.min)
  if (head !== null && Number.isFinite(head)) {
    ctx.fillStyle = '#ff9f0a'
    ctx.fillRect(40 + Math.max(0, Math.min(1, head)) * 19, 46, 1, 10)
  }
  ctx.restore()

  // 6. VU meter
  for (let i = 0; i < 8; i++) {
    ctx.fillStyle = level * 8 > i ? (i > 5 ? '#ff453a' : '#30d158') : 'rgba(0,0,0,0.18)'
    ctx.fillRect(33 + i * 4.3, 65.5, 3, 3)
  }

  // 7. recording light while listening
  if (input.phase === 'listening') {
    ctx.save()
    ctx.shadowColor = 'rgba(255,59,48,0.8)'
    ctx.shadowBlur = 3
    ctx.fillStyle = blink < 0.5 ? '#ff3b30' : '#7a1c17'
    ctx.beginPath(); ctx.arc(84, 68, 2, 0, Math.PI * 2); ctx.fill()
    ctx.restore()
  }

  // 8. screws
  ctx.fillStyle = 'rgba(0,0,0,0.35)'
  for (const [x, y] of SCREWS) { ctx.beginPath(); ctx.arc(x, y, 1.1, 0, Math.PI * 2); ctx.fill() }

  // 9. hover rim
  if (glow > 0.01) {
    ctx.strokeStyle = `rgba(255,255,255,${(0.45 * glow).toFixed(3)})`
    ctx.lineWidth = 1
    ctx.beginPath(); ctx.roundRect(6, 24, 88, 54, 8); ctx.stroke()
  }

  return moving || showing || bump > 0 || Math.abs(glowTarget - glow) > 0.01
})
