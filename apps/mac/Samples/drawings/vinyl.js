// visual.md A8 example 4: a vinyl record using the shown cover art.
let angle = 0, slide = 0

peek.on('enter', () => { slide = 1 })

peek.frame((ctx, input) => {
  const art = input.show?.elements.find(e => e.type === 'image')
  const playing = input.phase === 'speaking' || input.phase === 'showing'
  angle += (playing ? 1.2 : 0) * input.dt
  slide  = Math.max(0, slide - input.dt * 2.5)
  const ease = 1 - Math.pow(1 - (1 - slide), 3)   // ease-out

  ctx.save()
  ctx.translate(50 + (1 - ease) * 30, 50)          // slides in from the right on enter
  ctx.rotate(angle)

  // record
  ctx.fillStyle = '#111'
  ctx.beginPath(); ctx.arc(0, 0, 44, 0, Math.PI * 2); ctx.fill()
  ctx.strokeStyle = 'rgba(255,255,255,0.06)'; ctx.lineWidth = 0.6
  for (let r = 20; r < 44; r += 3) { ctx.beginPath(); ctx.arc(0, 0, r, 0, Math.PI * 2); ctx.stroke() }

  // label = cover art clipped to a circle
  ctx.save()
  ctx.beginPath(); ctx.arc(0, 0, 17, 0, Math.PI * 2); ctx.clip()
  if (art) ctx.drawImage(art.image, -17, -17, 34, 34)
  else { ctx.fillStyle = '#c8352b'; ctx.fillRect(-17, -17, 34, 34) }
  ctx.restore()

  ctx.fillStyle = '#000'
  ctx.beginPath(); ctx.arc(0, 0, 1.5, 0, Math.PI * 2); ctx.fill()
  ctx.restore()

  return playing || slide > 0
})
