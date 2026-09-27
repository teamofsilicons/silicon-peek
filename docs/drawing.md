# Drawing the visual

Every bubble has a circle that belongs to its Silicon: the **visual**. You fill it with a small JavaScript file that peek runs every frame. The API feels like HTML `<canvas>` 2D, plus a few peek-only extensions for Liquid Glass, blur and vibrancy. This page is the complete contract for writing that file.

```text
you write JS  ──►  peek runs it every frame  ──►  JS calls ctx.* (canvas style)
                                                   │
                            peek records calls ◄───┘
                                   │
               ┌───────────────────┴───────────────────┐
               ▼                                       ▼
    normal drawing → Core Graphics            glass / blur → native macOS views
               └───────────────────┬───────────────────┘
                                   ▼
                  stacked inside your position's panel
```

- You write **JavaScript** (ES2023), not Swift and not JSON.
- The script **only draws**. It reads everything happening in its bubble (speech, microphone level, show, ask, pointer, backdrop), reacts to clicks, and **sends nothing out**. There is no network, no file access, and no channel back to you.
- Peek owns everything outside the visual: the information arc, pills, the question arc, the mic and keyboard buttons and the down-arrow. A drawing cannot change them.
- There is no bubble background. The panel is invisible, so whatever your drawing leaves transparent shows the Carbon's desktop (or the window behind it). Draw your own backing, for example a glass disc, if you want one.

## Lifecycle and CLI

```bash
peek register side 5                       # claim a position (1 = top centre, clockwise)
peek register drawing ./logo.js            # validate + store + activate the drawing
peek send --speak "hi" --show '{…}'        # the bubble slides in, the drawing runs
peek unregister                            # position, drawing and shortcut are released
```

| Event | What happens to the drawing |
|---|---|
| `register drawing ./x.js` | Validated (see Validation below). On success it is stored, loaded, its top-level code runs once, then it is **paused**. |
| `register drawing` again | The old script is thrown away; the new one loads fresh. All state is lost. |
| `register side N` | The bubble moves. **The same script instance keeps running**, and top-level variables survive. `input.slot` changes and the `move` event fires. |
| `peek send …` | Resumes: `enter` event → frames → … → `leave` event → paused. |
| `unregister` | Script destroyed; the stored copies (on the Mac and on the server) are deleted. |
| Peek.app restarts | The script reloads from the top. State does **not** survive restarts. |

`peek send` fails with `drawing_not_registered` until a drawing is registered.

Useful flags:

```bash
peek register drawing ./logo.js --preview out.png     # also write a PNG grid of test frames
peek register drawing ./logo.js --dump-frame 30       # print test frame 30's display list (JSON; frames are 0–89)
peek register drawing ./logo.js --check               # validate only; don't replace the active drawing
```

The file is resolved against your current directory and the CLI reads it, so Peek.app never opens your files. The active drawing is stored at `~/Library/Application Support/Peek/drawings/<context>/<org_id>/<actor_id>/<sha256>.js` (the previous one is kept for rollback), and a copy is kept on the peek backend. When you register a position on a Mac that has no local copy, peek fetches the server copy and validates it before using it.

## The drawing area

- The script always draws in a **fixed 100 × 100 unit square**. `(0, 0)` is top-left and **y points down**, like canvas.
- **The area cannot be resized.** Peek scales it to pixels for Retina, normal mode and compact mode. The script never deals with pixels.
- The **visual circle** is the circle inscribed in the square: centre `(50, 50)`, radius `50`. Keep important content inside it; in some positions peek's arc and buttons cover the square's corners.
- Anything drawn outside `0…100` is clipped.
- All sizes (`lineWidth`, font size, radii) are in units. `ctx.font = '8px SF Pro'` means 8 units tall.
- Angles are in **radians**, like canvas: `0` points right, positive is clockwise (because y points down).

```text
(0,0) ┌────────────────────┐ (100,0)
      │     ╭────────╮     │
      │   ╭╯          ╰╮   │
      │   │   (50,50)   │   │   ← visual circle, r = 50
      │   ╰╮          ╭╯   │
      │     ╰────────╯     │
(0,100)└────────────────────┘ (100,100)
```

## Script structure

A drawing is **one `.js` file** (ES2023, at most 256 KiB, no imports). Top-level code runs once when it loads. Top-level variables are the script's memory between frames.

```js
// top level: runs once
let spin = 0

peek.on('click', e => { /* one-off reaction */ })

peek.frame((ctx, input) => {
  // runs every frame while the bubble is awake
  spin += input.dt
  // ... draw with ctx ...
  return true          // true = I'm animating, call me again next frame
})
```

### `peek.frame(fn)`

- `fn(ctx, input)` is called once per display frame (60 or 120 Hz) **while awake**.
- **Draw the whole visual every frame.** Nothing carries over from the last frame; peek clears the area before each call.
- **Return value:**
  - `true`: peek schedules another frame.
  - `false` or `undefined`: the drawing **sleeps**. The last frame stays on screen and costs nothing.
- A sleeping drawing wakes automatically for **one or more frames** when any of these happen:
  - `input.phase` changes;
  - a send arrives, or an answer is submitted;
  - `input.speech.level` or `input.mic.level` rises above 0;
  - hover starts or ends, or the pointer moves while hovering;
  - a click;
  - the slot, mode, appearance or backdrop changes.

  So a drawing that only reacts to voice can return `false` and still animate while someone is talking.

### `peek.on(event, fn)`

One-off moments. Continuous values live in `input`.

| Event | Payload | Fires when |
|---|---|---|
| `enter` | – | the bubble is about to slide in (see below) |
| `leave` | – | the bubble starts sliding out |
| `send` | the new `input.show` / `input.ask` / `input.speech` | a `peek send` arrives |
| `answer` | `{ value, via }` | the Carbon submits an answer (`via`: `voice`, `keyboard` or `click`) |
| `click` | `{ x, y, count }` | the Carbon clicks the visual (units; `count` 2 = double click) |
| `move` | `{ from, to }` | you moved to another position |

A click on the visual **does nothing else**. It exists only so the drawing can animate.

**`enter` comes shortly before the slide.** peek prepares each bubble out of sight first, so the glass, the backdrop colours and your first frame are ready when it lands: it fires `enter` and renders your frames off screen for about 0.35 s (at most 0.6 s), then slides the bubble in. An entrance animation you start on `enter` should therefore last longer than that, or it is over before the Carbon sees it.

Any other event name throws a `TypeError` that lists the valid ones. The one exception is the old `word` event from early drafts: registering it loads with a one-time warning in `logs`, and it never fires, because peek has no word timing (use `input.speech.progress` and `input.speech.level`).

### `peek.log(...args)`

Debug output. Shown in `register drawing` output and in Simulation. Ignored during normal operation.

## The `input` reference

`input` is a fresh read-only snapshot every frame.

```ts
type Input = {
  t: number                 // seconds since the script was loaded
  dt: number                // seconds since the previous frame (0 on the first frame after waking; capped at 0.1)

  slot: {
    index: 1|2|3|4|5|6|7|8  // 1 = top centre, clockwise
    side: 'top' | 'top-right' | 'right' | 'bottom-right'
        | 'bottom' | 'bottom-left' | 'left' | 'top-left'
    facing: number          // radians: direction from the bubble toward the screen centre
  }

  mode: 'normal' | 'compact'           // compact = drawn very small; simplify
  appearance: 'light' | 'dark'         // system appearance
  context: 'production' | 'testing' | 'simulation'   // testing bubbles also get a TEST pill from peek
  glass: 'live' | 'frosted'            // 'frosted' when real Liquid Glass is unavailable (see Glass below)
  backdrop: {
    tone: 'light' | 'dark'             // what the bubble is sitting on
    luminance: number                  // 0 (black) … 1 (white)
    color: string                      // average colour under the bubble, '#rrggbb'
    ink: '#ffffff' | '#000000'         // suggested high-contrast colour
    source: 'screen' | 'wallpaper' | 'appearance'
  }

  phase: 'hidden' | 'entering' | 'showing' | 'asking'
       | 'speaking' | 'listening' | 'typing' | 'transcribing' | 'leaving'

  hover: boolean                       // pointer is over the bubble
  mouse: {
    x: number, y: number               // pointer in drawing units (can be outside 0…100)
    dist: number                       // distance from (50,50), in units
    angle: number                      // radians from (50,50) to the pointer
    inside: boolean                    // pointer is inside the visual circle
  }

  speech: null | {                     // peek speaking your --speak text
    text: string
    level: number                      // 0…1, loudness of the audio playing right now
    progress: number                   // 0…1, played audio frames / total frames
    done: boolean                      // true once the last audio has played
  }

  mic: {
    level: number                      // 0…1, the Carbon's mic loudness (0 when not listening)
  }

  typing: null | { text: string }      // the Carbon's in-progress typed text

  show: null | {
    elements: Array<
      | { type: 'text', text: string }
      | { type: 'image', image: ImageHandle, caption: string | null,
          colors: { dominant: string, palette: string[] } }
    >
  }

  ask: null | {
    question: string
    type: 'text' | 'single_choice' | 'multiple_choice' | 'slider' | 'range'
    options?: Array<{ id: string, label: string, image: ImageHandle | null,
                      colors: { dominant: string, palette: string[] } | null }>
    min?: number, max?: number, step?: number
    value: any                          // live: current selection / slider value / range [a,b] / text
    highlight: string | null            // option id the Carbon is pointing at (pointer or keyboard)
  }
}
```

Notes:

- **`speech.level` versus `mic.level`**: `speech` is peek talking, `mic` is the Carbon talking. For "react to voice", use `Math.max(input.speech?.level ?? 0, input.mic.level)`.
- **`speech.progress`** comes from the audio clock, so it tracks what the Carbon actually hears. Before the whole speech has arrived it is an estimate that only moves forward and stays below 1.
- **There is no live transcript and no word timing.** A voice answer is transcribed once, after the Carbon stops; during that wait `phase` is `'transcribing'` (choice, slider and range asks only).
- **`backdrop`** is peek's estimate of what is behind the bubble. Use `backdrop.ink` for strokes and text that must stand out, and `appearance` for overall light or dark styling.
- **`ImageHandle`** is an opaque object. Its only use is `ctx.drawImage(handle, …)`; it also has `.width` and `.height`. File paths are never exposed. Handles become invalid when the next send replaces the content, and drawing an invalid handle does nothing.
- `typing.text` is the Carbon's in-progress input. The drawing can see it because it has no way to send it anywhere.

## The `ctx` reference

### Supported (HTML canvas 2D subset)

| Group | API |
|---|---|
| State | `save()`, `restore()` |
| Transform | `translate(x,y)`, `rotate(a)`, `scale(x,y)`, `transform(a,b,c,d,e,f)`, `setTransform(...)`, `resetTransform()` |
| Paths | `beginPath()`, `closePath()`, `moveTo`, `lineTo`, `arc`, `arcTo`, `ellipse`, `rect`, `roundRect(x,y,w,h,r)`, `quadraticCurveTo`, `bezierCurveTo` |
| Path2D | `new Path2D()`, `new Path2D('M0 0 L10 10 …')` (SVG path strings), and passing a `Path2D` to `fill` / `stroke` / `clip` |
| Painting | `fill(rule?)`, `stroke()`, `fillRect`, `strokeRect`, `clearRect`, `clip(rule?)` where `rule` is `'nonzero'` or `'evenodd'` |
| Style | `fillStyle`, `strokeStyle` (CSS colours or gradients), `lineWidth`, `lineCap`, `lineJoin`, `miterLimit`, `setLineDash`, `lineDashOffset`, `globalAlpha`, `globalCompositeOperation` (`source-over`, `multiply`, `screen`, `overlay`, `destination-out`, `lighter`) |
| Gradients | `createLinearGradient`, `createRadialGradient`, `createConicGradient`, `addColorStop` |
| Shadows | `shadowColor`, `shadowBlur`, `shadowOffsetX/Y` |
| Filter | `filter = 'blur(Npx)'` only (blurs your own drawing, not the desktop) |
| Text | `font`, `textAlign`, `textBaseline`, `fillText`, `strokeText`, `measureText` (returns `width`, `actualBoundingBoxLeft/Right/Ascent/Descent` and `fontBoundingBoxAscent/Descent`) |
| Images | `drawImage(handle, dx, dy, dw, dh)` and the 9-argument crop form |

Fonts: `SF Pro`, `SF Pro Rounded`, `SF Mono`, `New York`, `system-ui`, weights `100` to `900`. Nothing else is loaded.

### Not supported

`getImageData`, `putImageData`, `toDataURL`, `createPattern`, `isPointInPath` (do hit-testing with math), any DOM, `fetch`, `XMLHttpRequest`, `setTimeout`/`setInterval` (use `input.t` and `input.dt`), `require`, `import`, `WebAssembly`. `Math.random` works.

### Peek extensions

These use native macOS materials, which see what is **behind the window**: the wallpaper and other apps. Ordinary canvas drawing cannot.

```ts
ctx.fillGlass(opts?)   // fill the current path (or a Path2D) with Liquid Glass
ctx.fillGlass(path2d, opts?)
  opts = {
    style?: 'regular' | 'clear'        // default 'regular'. 'clear' lenses strongly; 'regular' is frosted
    tint?: string                      // CSS colour, optional
    interactive?: boolean              // glass reacts to the pointer (default false)
    rule?: 'nonzero' | 'evenodd'       // default 'nonzero'; use 'evenodd' for holes
  }

ctx.fillBlur(opts?)    // fill the current path (or a Path2D) with a frosted material
ctx.fillBlur(path2d, opts?)
  opts = {
    material?: 'hud' | 'popover' | 'menu' | 'sidebar' | 'underWindow'   // default 'hud'
    rule?: 'nonzero' | 'evenodd'
  }

ctx.vibrant = true | false
  // While true, fills, strokes and text are drawn with system vibrancy:
  // their colour picks up from what's behind. Best for monochrome marks and text.
```

**Stacking rule:** calls stack in the order you make them. Anything drawn *before* a `fillGlass` sits under the glass and is refracted by it. Anything drawn *after* sits on top.

**Limit:** at most **3** glass or blur fills per frame. Extra ones are drawn as a flat translucent fill.

### Glass: what is cheap and what is not

Each glass fill is a real system view. Peek records the path **as you built it** (the local path) together with the current transform, and treats the two differently:

- **Transform-only changes apply every frame.** Drawing the same path under a different `translate`, `rotate` or `scale` only moves the existing glass view. That is cheap.
- **Outline and option changes are capped at 10 Hz per bubble.** A new path shape, or a new `style`, `tint`, `interactive` or `rule`, rebuilds the glass view; peek applies at most 10 such rebuilds per second, and the last write wins. Rebuilding every frame would cost about a third of a CPU core per bubble.
- `input.glass` is `'frosted'` on systems where peek cannot show live Liquid Glass. There, `style:'clear'` looks like `'regular'`, and content under the glass is blurred through rather than refracted. Use it to compensate, for example with a stronger outline.

## Rules for good drawings

1. **Keep glass outlines still; animate on top of them.** Build glass paths once at the top level. Move them with transforms if you must; never reshape them per frame. `register drawing` warns when a glass outline changes in more than 10% of test frames.
2. **Return `false` when nothing moves.** A sleeping drawing costs nothing, and peek wakes it for voice, hover, clicks and sends anyway.
3. **Use `input.dt` for motion**, not frame counts. Frame rate varies between 60 and 120 Hz.
4. **Smooth values over time.** Voice levels jump around. Use `v += (target - v) * Math.min(1, dt * 10)`.
5. **Use `backdrop.ink` or `ctx.vibrant`** for thin lines and text, so they stay readable on any wallpaper.
6. **Simplify in `compact` mode.** The drawing is tiny there. Drop text and thin details.
7. **Keep it inside the circle.**
8. **Do not assume `show`, `ask` or `speech` exist.** Use `?.` everywhere.

## Limits

| Limit | Value | When exceeded |
|---|---|---|
| Script size | 256 KiB | rejected at register (`drawing_too_large`) |
| Top-level code | 250 ms when the script loads | the load fails (`drawing_invalid`) |
| Frame time | 4 ms per `frame()` call (and per event handler) | frame dropped (the last frame stays); 30 overruns in 5 s → fallback visual |
| Ops per frame | 5,000 | extra ops ignored, warning logged |
| Glass + blur fills per frame | 3 | extra ones drawn as a flat translucent fill |
| Glass rebuilds (outline or options) | 10 per second per bubble | later writes wait; the last one wins |
| JS memory | 16 MB | script killed → fallback visual |
| Uncaught exception in `frame()` | – | frame dropped; 10 in a row → fallback visual |

**Fallback visual:** a plain glass circle with your initial. Your next `peek send`, `peek register side` or `peek status` carries a `drawing_fallback_active` warning with the error message and stack, once, so you can fix and re-register. Other commands do not carry it. Until you register again, `peek status` also shows `drawing.active: false` and the error in `drawing.last_error`.

## Examples

### Example 1: minimal (a dot that pulses with any voice)

```js
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
```

### Example 2: glass orb with a voice ring

```js
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
```

### Example 3: cassette (glass with holes, clipped tape, reels, clicks)

```js
const REELS = [[30, 50], [70, 50]]

// body with two reel holes, built once
const body = new Path2D()
body.roundRect(5, 21, 90, 58, 7)
for (const [x, y] of REELS) body.arc(x, y, 9, 0, Math.PI * 2)

let spin = 0, bump = 0

peek.on('click', () => { bump = 1 })

peek.frame((ctx, input) => {
  const voice  = Math.max(input.speech?.level ?? 0, input.mic.level)
  const moving = input.phase === 'speaking' || input.phase === 'listening'
  spin += (moving ? 2 + voice * 8 : 0) * input.dt
  bump  = Math.max(0, bump - input.dt * 4)

  // a light tint from the cover art, if one is being shown
  const art  = input.show?.elements.find(e => e.type === 'image')
  const tint = art ? art.colors.dominant + '40' : 'rgba(255,255,255,0.18)'

  // 1. clear glass body (same outline every frame → never rebuilt; tint changes only per send)
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
```

### Example 4: vinyl record using the shown cover art

```js
let angle = 0, slide = 0

peek.on('enter', () => { slide = 1 })

peek.frame((ctx, input) => {
  const art = input.show?.elements.find(e => e.type === 'image')
  const playing = input.phase === 'speaking' || input.phase === 'showing'
  angle += (playing ? 1.2 : 0) * input.dt
  slide  = Math.max(0, slide - input.dt * 1.25)   // 0.8 s: outlasts the off-screen start
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
```

### Example 5: reacting to an ask (a gauge that follows the slider live)

```js
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
```

### Example 6: an eye that looks inward, and at the pointer when hovered

```js
let lx = 0, ly = 0

peek.frame((ctx, input) => {
  const a = input.hover ? input.mouse.angle : input.slot.facing
  const d = input.hover ? Math.min(10, input.mouse.dist / 4) : 6
  lx += (Math.cos(a) * d - lx) * Math.min(1, input.dt * 10)
  ly += (Math.sin(a) * d - ly) * Math.min(1, input.dt * 10)

  // blink every ~4 s, and on loud peaks of the speech
  const blink = (input.t % 4) < 0.12 || (input.speech?.level ?? 0) > 0.6

  ctx.fillStyle = '#fff'
  ctx.beginPath(); ctx.ellipse(50, 50, 30, blink ? 2 : 22, 0, 0, Math.PI * 2); ctx.fill()

  if (!blink) {
    ctx.fillStyle = '#111'
    ctx.beginPath(); ctx.arc(50 + lx, 50 + ly, 9, 0, Math.PI * 2); ctx.fill()
  }

  return true   // always blinking
})
```

### Example 7: speech progress, transcribing and frosted glass

```js
const ring = new Path2D()
ring.arc(50, 50, 42, 0, Math.PI * 2)
let wait = 0

peek.frame((ctx, input) => {
  // thicker rim when live glass is unavailable, so the circle still reads as glass
  ctx.fillGlass(ring, { style: 'clear' })
  if (input.glass === 'frosted') {
    ctx.strokeStyle = input.backdrop.ink; ctx.globalAlpha = 0.25; ctx.lineWidth = 2
    ctx.stroke(ring); ctx.globalAlpha = 1
  }

  // how far through the speech we are
  const p = input.speech?.progress ?? 0
  ctx.strokeStyle = input.backdrop.ink; ctx.lineWidth = 3; ctx.lineCap = 'round'
  ctx.beginPath(); ctx.arc(50, 50, 36, -Math.PI / 2, -Math.PI / 2 + p * Math.PI * 2); ctx.stroke()

  // a spinner while a voice answer is being matched
  if (input.phase === 'transcribing') {
    wait += input.dt * 4
    ctx.beginPath(); ctx.arc(50, 50, 20, wait, wait + Math.PI * 1.2); ctx.stroke()
  }

  return input.phase === 'speaking' || input.phase === 'transcribing'
})
```

## Validation

Before a drawing is accepted, peek runs it **offscreen for 90 frames** inside Peek.app, with the same JavaScript engine and renderer used on screen, cycling through:

- every `phase`, including `transcribing`;
- `mode` normal and compact;
- `appearance` light and dark, `backdrop` light and dark;
- `context` production, testing and simulation, and `glass` live and frosted;
- `show` with a sample image and text, and an `ask` of each type with a moving `value`;
- `speech.level` and `mic.level` as sine waves, `speech.progress` running from 0 to 1 and `speech.done`;
- hover on and off, the pointer circling, one click, one `move`.

Test frames are numbered 0 to 89.

It **fails** if the script throws (while loading, in `frame()` or in an event handler), if its top-level code runs longer than 250 ms, if `frame()` is interrupted at the 4 ms limit in 5 test frames or its 95th-percentile time is over 4 ms, if it runs out of memory, is larger than 256 KiB or is not UTF-8, if it never calls `peek.frame`, or if it **draws nothing at all**. It **warns** (but accepts) with these `warnings[].code` values:

| Code | Why |
|---|---|
| `glass_outline_unstable` | a glass outline changed in more than 10% of the test frames |
| `text_in_compact` | text is drawn in compact mode |
| `ops_truncated` | a frame recorded more than 5,000 ops; the rest were ignored |
| `glass_limit` | more than 3 glass or blur fills in a frame; the extra ones were drawn flat |
| `ignored_value` | something peek ignored or clamped while drawing (for example an unsupported value), reported once per message |
| `dump_frame_unavailable` | `--dump-frame N` named a frame outside 0–89, or one that was interrupted |

Success:

```text
$ peek register drawing ./cassette.js
✓ loaded (3.1 KB)
✓ 90/90 frames ok   p50 0.31ms  p95 0.58ms  max 0.92ms
✓ ops/frame  max 212
✓ glass rebuilt 1 time in 90 frames
drawing active for silicon "dj" at slot 5 (bottom)
```

Failure (exit 4, `drawing_invalid`; the block goes to stderr and is followed by the usual `error:` and `hint:` lines):

```text
$ peek register drawing ./cassette.js
✗ frame 14 threw: TypeError: cannot read property 'colors' of undefined
    at cassette.js:18:34
      const tint = art.colors.dominant ?? '#e8d9b8'
                       ^
  input at frame 14: phase=showing, show=null
drawing NOT registered (previous drawing still active)
```

Warning:

```text
⚠ glass fill #1 changed outline in 90/90 frames. Glass outlines are applied at most 10 times per second, so it looks choppy and costs CPU. Keep glass outlines fixed and animate on top of them (https://peek.teamofsilicons.com/docs/drawing#glass-what-is-cheap-and-what-is-not).
```

With `--json`, the same warning is `{"code":"glass_outline_unstable","message":"glass fill #1 changed outline in 90/90 frames. …"}` in `warnings`, and `peek.log` lines are in `logs`.

For `--preview`, glass layers are drawn as a flat translucent approximation, because live glass needs a real window.

## Debugging: the display list

`--dump-frame N` adds what test frame N (0–89) turned into, which is what peek actually draws, as `dump` in the result (the human output prints it after the summary). Example (cassette, mid-speech):

```json
{
  "frame": 30,
  "again": true,
  "ops": 212,
  "dropped_ops": 0,
  "input": "phase=speaking, mode=normal, appearance=light, speech.level=0.62",
  "layers": [
    {
      "kind": "glass",
      "local_path": "M12 21 L88 21 C91.866 21 95 24.134 95 28 L95 72 C95 75.866 91.866 79 88 79 L12 79 C8.134 79 5 75.866 5 72 L5 28 C5 24.134 8.134 21 12 21 Z M39 50 C39 54.971 34.971 59 30 59 C25.029 59 21 54.971 21 50 C21 45.029 25.029 41 30 41 C34.971 41 39 45.029 39 50 Z …",
      "path": "M12 21 L88 21 C91.866 21 95 24.134 95 28 …",
      "transform": [1, 0, 0, 1, 0, 0],
      "rule": "evenodd",
      "style": "clear",
      "tint": "#c8352b40",
      "interactive": true,
      "hash": "g:7f3a91"
    },
    {
      "kind": "draw",
      "hash": "d:be2c04",
      "ops": [
        ["fillStyle", "rgba(255,255,255,0.6)"],
        ["beginPath"], ["roundRect", 13, 25, 74, 10, 2], ["fill", "nonzero"],
        ["fillStyle", "#1c1c1c"], ["font", "600 6px SF Pro"], ["textAlign", "center"],
        ["fillText", "SIDE A", 50, 32],
        ["save"],
        ["beginPath"], ["roundRect", 40, 45, 20, 10, 3], ["clip", "nonzero"],
        ["fillStyle", "#3a2a1a"],
        ["fillRect", 36.2, 48, 2.5, 4], ["fillRect", 41.2, 48, 2.5, 4],
        ["restore"],
        ["save"], ["translate", 30, 50], ["rotate", 4.71], ["scale", 1, 1],
        ["strokeStyle", "#1c1c1c"], ["lineWidth", 1.6], ["lineCap", "round"],
        ["beginPath"], ["arc", 0, 0, 3, 0, 6.283], ["stroke"],
        ["restore"]
      ]
    }
  ]
}
```

(Shortened here; the real dump lists every op.)

- `again` is what `frame()` returned; `ops` and `dropped_ops` count the recorded and ignored ops; `input` summarizes the test input of that frame.
- A new layer starts at every `fillGlass`, `fillBlur`, and every change of `ctx.vibrant`. `kind` is `glass`, `blur`, `draw` or `vibrant-draw`.
- A glass layer's `local_path` is the path **as you built it**, and `transform` is the canvas transform at the time of the call, `[a, b, c, d, e, f]`, which is applied to the glass view as a layer transform. `path` is the same outline with the transform applied (in drawing units). The `hash` covers the local path, the fill rule and the glass options only, so a transform change never rebuilds the glass.
- Paths are written with `M`, `L`, `Q`, `C` and `Z` commands, whatever you used to build them.
- `draw` ops keep canvas-style state (`save`, `translate`, …) and are replayed as they are. Path2D objects appear as `{"path2d":[…]}`, gradients as `{"type","params","stops"}`, and images as `{"image":<id>}`.
- If a layer's `hash` matches the previous frame, peek does not redraw it.

## Next

- Try your drawing in Peek's **Simulation** window (menu bar → Simulation). It shows `peek.log` output and every combination of inputs, without IAM or Ting.
- [Speak and show](show.md) and [Ask a question](ask.md) describe what fills `input.show` and `input.ask`.
