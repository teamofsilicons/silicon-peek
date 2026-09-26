# Visual: how a drawing works

This document is the technical spec for the **visual area** of a peek bubble: the drawable circle that belongs to a silicon. It has two audiences:

- **Part A — Writing a drawing.** For silicons (agents) that author `logo.js`.
- **Part B — Rendering a drawing.** For agents building peek itself (Swift / macOS).

Read `understanding.md` first for what peek is. This file only covers the visual.

---

## 0. Mental model

```
silicon writes JS  ──►  peek runs it every frame  ──►  JS calls ctx.* (HTML-canvas style)
                                                         │
                                  peek records calls ◄───┘
                                         │
                     ┌───────────────────┴───────────────────┐
                     ▼                                       ▼
          normal drawing → CGContext                glass / blur → native macOS views
                     └───────────────────┬───────────────────┘
                                         ▼
                        stacked inside the slot's NSPanel
                                         ▼
                       WindowServer composites to screen
```

- The silicon **writes JavaScript**, not Swift and not JSON.
- The script **only draws**. It reads everything happening in its bubble (speech, mic, show, ask, mouse, backdrop), reacts to clicks, and **sends nothing out**. There is no network, no file access, and no channel back to the silicon.
- The API feels like HTML `<canvas>` 2D, plus a few peek-only extensions for **Liquid Glass**, **blur** and **vibrancy**.
- Peek owns everything outside the visual area: the information arc, pills, question arc, mic/keyboard buttons and down-arrow. A drawing can't change them.

---

# Part A — Writing a drawing

## A1. Lifecycle & CLI

```bash
peek register side 5                       # claim a slot (1 = top center, clockwise)
peek register drawing ./logo.js            # validate + store + activate the drawing
peek send --speak "hi" --show '{...}'      # bubble slides in, drawing runs
peek unregister                            # slot, drawing and shortcut are released
```

| Event | What happens to the drawing |
|---|---|
| `register drawing ./x.js` | Validated (see A9). On success it's stored, loaded, top-level code runs once, then it's **paused**. |
| `register drawing` again | Old script is thrown away; the new one loads fresh. All state is lost. |
| `register side N` | The bubble moves. **The same script instance keeps running**, and top-level variables survive. `input.slot` changes and the `move` event fires. |
| `peek send ...` | Resumes: `enter` event → frames → … → `leave` event → paused. |
| `unregister` | Script destroyed, cached copy deleted. |
| peek app restart | Script reloads from the top. State does **not** persist across restarts. |

`peek send` fails with an error if the silicon has no side or no drawing registered.

Useful flags:

```bash
peek register drawing ./logo.js --preview out.png     # also write a PNG grid of test frames
peek register drawing ./logo.js --dump-frame 30       # print frame 30's display list (JSON)
peek register drawing ./logo.js --check               # validate only; don't replace the active drawing
```

## A2. The drawing area

- The script always draws in a **fixed 100 × 100 unit square**. `(0, 0)` is top-left, **y points down**, like canvas.
- **The area can't be resized** by the silicon. Peek scales it to pixels for Retina, normal mode and compact mode. The script never deals with pixels.
- The **visual circle** is the circle inscribed in the square: center `(50, 50)`, radius `50`. Keep important content inside it. In some slots peek's arc and buttons may cover the square's corners.
- Anything drawn outside `0…100` is clipped.
- All sizes (`lineWidth`, font size, radii) are in units. `ctx.font = '8px SF Pro'` means 8 units tall.
- Angles are in **radians**, like canvas. `0` = pointing right, positive = clockwise (because y points down).

```
(0,0) ┌────────────────────┐ (100,0)
      │     ╭────────╮     │
      │   ╭╯          ╰╮   │
      │   │   (50,50)   │   │   ← visual circle, r = 50
      │   ╰╮          ╭╯   │
      │     ╰────────╯     │
(0,100)└────────────────────┘ (100,100)
```

## A3. Script structure

A drawing is **one `.js` file** (ES2023, max 256 KB, no imports). Top-level code runs once when loaded. Top-level variables are the script's memory between frames.

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
- **Draw the whole visual every frame.** Nothing carries over from the last frame. Peek clears the area before each call.
- **Return value:**
  - `true` → peek schedules another frame.
  - `false` / `undefined` → the drawing **sleeps**. The last frame stays on screen and costs nothing.
- A sleeping drawing wakes automatically for **one or more frames** when any of these happen:
  - `input.phase` changes
  - a `send` arrives, a word is spoken, or an answer is submitted
  - `input.speech.level` or `input.mic.level` goes above 0
  - hover starts or ends, or the mouse moves while hovering
  - a click
  - the slot, mode, appearance or backdrop changes

  So a drawing that only reacts to voice can return `false` and still animate while someone is talking.

### `peek.on(event, fn)`

One-off moments. Continuous values live in `input`.

| Event | Payload | Fires when |
|---|---|---|
| `enter` | — | the bubble starts sliding in |
| `leave` | — | the bubble starts sliding out |
| `send` | the new `input.show` / `input.ask` / `input.speech` | a `peek send` arrives |
| `word` | `{ word, index }` | text-to-speech starts speaking a word |
| `answer` | `{ value, via }` | the user submits an answer (`via`: `voice`, `keyboard` or `click`) |
| `click` | `{ x, y, count }` | the user clicks the visual (units; `count` 2 = double click) |
| `move` | `{ from, to }` | the silicon moved to another slot |

A click on the visual **does nothing else**. It exists only so the drawing can animate.

### `peek.log(...args)`

Debug output. Shown in `register drawing` validation output and in simulation mode. Ignored during normal operation.

## A4. `input` reference

`input` is a fresh read-only snapshot every frame.

```ts
type Input = {
  t: number                 // seconds since the script was loaded
  dt: number                // seconds since the previous frame (0 on the first frame after waking)

  slot: {
    index: 1|2|3|4|5|6|7|8  // 1 = top center, clockwise
    side: 'top' | 'top-right' | 'right' | 'bottom-right'
        | 'bottom' | 'bottom-left' | 'left' | 'top-left'
    facing: number          // radians: direction from the bubble toward the screen center
  }

  mode: 'normal' | 'compact'           // compact = drawn very small; simplify
  appearance: 'light' | 'dark'         // system appearance
  backdrop: {
    tone: 'light' | 'dark'             // what the bubble is sitting on
    luminance: number                  // 0 (black) … 1 (white)
    color: string                      // average color under the bubble, '#rrggbb'
    ink: '#ffffff' | '#000000'         // suggested high-contrast color
    source: 'screen' | 'wallpaper' | 'appearance'
  }

  phase: 'hidden' | 'entering' | 'showing' | 'asking'
       | 'speaking' | 'listening' | 'typing' | 'leaving'

  hover: boolean                       // pointer is over the bubble
  mouse: {
    x: number, y: number               // pointer in drawing units (can be outside 0…100)
    dist: number                       // distance from (50,50), in units
    angle: number                      // radians from (50,50) to the pointer
    inside: boolean                    // pointer is inside the visual circle
  }

  speech: null | {                     // peek speaking the silicon's --speak text
    text: string
    level: number                      // 0…1, loudness right now
    word: string | null                // word being spoken right now
    progress: number                   // 0…1 through the text
    done: boolean
  }

  mic: {
    level: number                      // 0…1, user's mic loudness (0 when not listening)
    transcript: string                 // live partial transcript ('' when not listening)
  }

  typing: null | { text: string }      // user's in-progress typed text

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
    highlight: string | null            // option id the user is pointing at or saying
  }
}
```

Notes:
- **`speech.level` vs `mic.level`**: `speech` is peek talking, `mic` is the user talking. For "react to voice", use `Math.max(input.speech?.level ?? 0, input.mic.level)`.
- **`backdrop`** is peek's estimate of what's behind the bubble. Use `backdrop.ink` for strokes and text that need to stand out, and `appearance` for overall light or dark styling.
- **`ImageHandle`** is an opaque object. Its only use is `ctx.drawImage(handle, …)`. It also has `.width` and `.height`. File paths are never exposed.
- `typing.text` and `mic.transcript` are the user's in-progress input. The drawing can see them because it has no way to send them anywhere.

## A5. `ctx` reference

### Supported (HTML canvas 2D subset)

| Group | API |
|---|---|
| State | `save()`, `restore()` |
| Transform | `translate(x,y)`, `rotate(a)`, `scale(x,y)`, `transform(a,b,c,d,e,f)`, `setTransform(...)`, `resetTransform()` |
| Paths | `beginPath()`, `closePath()`, `moveTo`, `lineTo`, `arc`, `arcTo`, `ellipse`, `rect`, `roundRect(x,y,w,h,r)`, `quadraticCurveTo`, `bezierCurveTo` |
| Path2D | `new Path2D()`, `new Path2D('M0 0 L10 10 …')` (SVG path strings), and passing a `Path2D` to `fill` / `stroke` / `clip` |
| Painting | `fill(rule?)`, `stroke()`, `fillRect`, `strokeRect`, `clearRect`, `clip(rule?)` where `rule` = `'nonzero' \| 'evenodd'` |
| Style | `fillStyle`, `strokeStyle` (CSS colors or gradients), `lineWidth`, `lineCap`, `lineJoin`, `miterLimit`, `setLineDash`, `lineDashOffset`, `globalAlpha`, `globalCompositeOperation` (`source-over`, `multiply`, `screen`, `overlay`, `destination-out`, `lighter`) |
| Gradients | `createLinearGradient`, `createRadialGradient`, `createConicGradient`, `addColorStop` |
| Shadows | `shadowColor`, `shadowBlur`, `shadowOffsetX/Y` |
| Filter | `filter = 'blur(Npx)'` only (blurs your own drawing, not the desktop) |
| Text | `font`, `textAlign`, `textBaseline`, `fillText`, `strokeText`, `measureText` (returns `width`) |
| Images | `drawImage(handle, dx, dy, dw, dh)` and the 9-argument crop form |

Fonts: `SF Pro`, `SF Pro Rounded`, `SF Mono`, `New York`, `system-ui`. Weights from `100` to `900`. Nothing else is loaded.

### Not supported

`getImageData`, `putImageData`, `toDataURL`, `createPattern`, `isPointInPath` (do hit-testing yourself with math), any DOM, `fetch`, `XMLHttpRequest`, `setTimeout`/`setInterval` (use `input.t` and `input.dt`), `require`, `import`, `WebAssembly`. `Math.random` works.

### Peek extensions

These use native macOS materials, which see what's **behind the window**: the wallpaper and other apps. Ordinary canvas drawing can't do that.

```ts
ctx.fillGlass(opts?)   // fill the current path (or a Path2D) with Liquid Glass
ctx.fillGlass(path2d, opts?)
  opts = {
    style?: 'regular' | 'clear'        // default 'regular'
    tint?: string                      // CSS color, optional
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
  // their color picks up from what's behind. Best for monochrome marks and text.
```

**Stacking rule:** calls are stacked in the order you make them. Anything drawn *before* a `fillGlass` sits under the glass and is refracted by it. Anything drawn *after* sits on top.

**Limits:** at most **3** glass/blur fills per frame.

## A6. Rules for good drawings

1. **Keep glass outlines still. Animate on top of them.** Each glass fill is a real system view. Peek skips rebuilding it when the outline and options are identical to the previous frame, which makes it free. A glass outline that changes every frame is expensive and may look wobbly. Moving the reels *on top of* the glass is cheap; spinning the glass itself is not. `register drawing` warns if glass changes on every test frame.
2. **Return `false` when nothing is moving.** A sleeping drawing costs nothing, and peek wakes it for voice, hover, clicks and sends anyway (A3).
3. **Use `input.dt` for motion**, not frame counts. Frame rate varies between 60 and 120 Hz.
4. **Smooth values over time.** Voice levels jump around. Use `v += (target - v) * Math.min(1, dt * 10)`.
5. **Use `backdrop.ink` or `ctx.vibrant`** for thin lines and text, so they're readable on any wallpaper.
6. **Simplify in `compact` mode.** The drawing is tiny there. Drop text and thin details.
7. **Keep it inside the circle** (A2).
8. **Don't assume `show`, `ask` or `speech` exist.** Use `?.` everywhere.

## A7. Limits

| Limit | Value | When exceeded |
|---|---|---|
| Script size | 256 KB | rejected at register |
| Frame time | 4 ms per `frame()` call | frame dropped (last frame stays); 30 overruns in 5 s → fallback visual |
| Ops per frame | 5,000 | extra ops ignored, warning logged |
| Glass + blur fills per frame | 3 | extra ones drawn as a flat translucent fill |
| JS memory | 16 MB | script killed → fallback visual |
| Uncaught exception in `frame()` | — | frame dropped; 10 in a row → fallback visual |

**Fallback visual:** a plain glass circle with the silicon's initial. The silicon sees the error the next time it runs any `peek` command.

## A8. Examples

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

  // tint from the cover art if one is being shown
  const art  = input.show?.elements.find(e => e.type === 'image')
  const tint = art?.colors.dominant ?? '#e8d9b8'

  // 1. glass body (same outline every frame → never rebuilt; tint changes only per send)
  ctx.fillGlass(body, { tint, interactive: true, rule: 'evenodd' })

  if (input.mode === 'compact') return moving || bump > 0   // tiny: glass only

  // 2. label shows the word being spoken
  ctx.fillStyle = 'rgba(255,255,255,0.6)'
  ctx.beginPath(); ctx.roundRect(13, 25, 74, 10, 2); ctx.fill()
  ctx.fillStyle = '#1c1c1c'
  ctx.font = '600 6px SF Pro'
  ctx.textAlign = 'center'
  ctx.fillText(input.speech?.word ?? 'SIDE A', 50, 32)

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

### Example 6: eye that looks inward, and at the pointer when hovered

```js
let lx = 0, ly = 0

peek.frame((ctx, input) => {
  const a = input.hover ? input.mouse.angle : input.slot.facing
  const d = input.hover ? Math.min(10, input.mouse.dist / 4) : 6
  lx += (Math.cos(a) * d - lx) * Math.min(1, input.dt * 10)
  ly += (Math.sin(a) * d - ly) * Math.min(1, input.dt * 10)

  // blink every ~4s, and on every spoken word
  const blink = (input.t % 4) < 0.12 || (input.speech?.word && (input.t * 8) % 1 < 0.2)

  ctx.fillStyle = '#fff'
  ctx.beginPath(); ctx.ellipse(50, 50, 30, blink ? 2 : 22, 0, 0, Math.PI * 2); ctx.fill()

  if (!blink) {
    ctx.fillStyle = '#111'
    ctx.beginPath(); ctx.arc(50 + lx, 50 + ly, 9, 0, Math.PI * 2); ctx.fill()
  }

  return true   // always blinking
})
```

## A9. Validation (`peek register drawing`)

Before a drawing is accepted, peek runs it **offscreen for 90 frames**, cycling through:

- every `phase`
- `mode` normal and compact
- `appearance` light and dark, `backdrop` light and dark
- `show` with a sample image + text, `ask` of each type with a moving `value`
- `speech.level` and `mic.level` as sine waves, words firing
- hover on/off, pointer circling, one click, one `move`

It fails if the script throws, goes over the time limit at the 95th percentile, exceeds a limit (A7), or **draws nothing at all**. It warns (but accepts) if glass outlines change every frame or text is drawn in compact mode.

Success:

```
$ peek register drawing ./cassette.js
✓ loaded (3.1 KB)
✓ 90/90 frames ok   p50 0.31ms  p95 0.58ms  max 0.92ms
✓ ops/frame  max 212
✓ glass fills 1 (stable: rebuilt 1 time in 90 frames)
drawing active for silicon "dj" at slot 5 (bottom)
```

Failure:

```
$ peek register drawing ./cassette.js
✗ frame 14 threw: TypeError: cannot read property 'colors' of undefined
    at cassette.js:18:34
      const tint = art.colors.dominant ?? '#e8d9b8'
                       ^
  input at frame 14: phase=showing, show=null
drawing NOT registered (previous drawing still active)
```

Warning:

```
⚠ glass fill #1 changed outline in 90/90 frames. Glass is rebuilt every frame, which is slow.
  Keep glass outlines fixed and animate on top of them. (see visual.md A6.1)
```

## A10. Debugging: the display list

`--dump-frame N` prints what your frame turned into, which is what peek actually draws. Example (cassette, mid-speech):

```json
{
  "frame": 30,
  "again": true,
  "layers": [
    {
      "kind": "glass",
      "path": "M12 21 H88 A7 7 0 0 1 95 28 V72 A7 7 0 0 1 88 79 H12 A7 7 0 0 1 5 72 V28 A7 7 0 0 1 12 21 Z M39 50 A9 9 0 1 1 21 50 A9 9 0 1 1 39 50 Z M79 50 A9 9 0 1 1 61 50 A9 9 0 1 1 79 50 Z",
      "rule": "evenodd",
      "style": "regular",
      "tint": "#c8352b",
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
        ["fillText", "Prateek", 50, 32],
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

(Output shortened here; the real dump lists every op.)

- A new layer starts at every `fillGlass`, `fillBlur`, and every change of `ctx.vibrant`.
- Glass `path`s are **flattened**: the current transform has already been applied, so they're plain paths in 100 × 100 space.
- `draw` ops keep canvas-style state (`save`, `translate`, …) and are replayed as they are.
- If a layer's `hash` matches the previous frame, peek doesn't redraw it.

---

# Part B — Rendering a drawing (for building peek)

## B1. Components

```
PeekApp
 ├─ SlotManager            8 slots, silicon ↔ slot, slide in/out, shortcuts
 ├─ InputHub               one shared source: mic, TTS, mouse, appearance, backdrop
 ├─ DrawingHost (×1 per registered silicon)
 │    ├─ JSRuntime         QuickJS runtime + context on its own thread
 │    ├─ Recorder          the `ctx` object; writes a binary op buffer
 │    ├─ FrameScheduler    display link, wake/sleep, time budget
 │    └─ Compositor        splits → diffs → renders layers into the SlotView
 └─ SlotPanel (NSPanel, ×1 per active slot)
      └─ SlotView
           ├─ VisualView   the 100×100 drawing area (glass views + draw layers)
           └─ Chrome       arc, pills, question arc, mic/keyboard, down-arrow (SwiftUI, peek-only)
```

## B2. Window: one `NSPanel` per slot

```swift
let panel = NSPanel(contentRect: frame,
                    styleMask: [.borderless, .nonactivatingPanel],
                    backing: .buffered, defer: false)
panel.isOpaque = false
panel.backgroundColor = .clear
panel.hasShadow = false                  // shadows are drawn per element, not per window
panel.level = .floating
panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary]
panel.isMovable = false
panel.hidesOnDeactivate = false
```

- **Non-activating** is required: clicking a bubble must never take focus from the user's app. Override `canBecomeKey` to return `true` **only** while typing mode is active (see `understanding.md`: `\`, typing, Esc).
- **Size the panel for its largest state** (visual + arc + expanded mic/keyboard) from the start. **Never resize the panel during an animation**, because window resizes stutter. Animate the contents with layers instead.
- **Slide in/out** by animating the panel's content offset from beyond the screen edge, using a spring with slight overshoot. Use `NSScreen.visibleFrame` so top and bottom slots slide out from the menu bar and Dock edges, not from behind them.
- **Clicks pass through empty areas:** track the pointer (B8) and set `panel.ignoresMouseEvents = !isOverContent(point)` as it moves. Transparent pixels alone don't reliably pass clicks through, so don't rely on that.

## B3. JS runtime: QuickJS

Use **QuickJS**, embedded as a C library, one `JSRuntime` + `JSContext` per registered drawing, each on its own thread. Why QuickJS rather than JavaScriptCore:
- `JS_SetInterruptHandler` → reliably stops a script that runs over its time budget. Public JavaScriptCore has no supported way to do this.
- `JS_SetMemoryLimit(rt, 16 << 20)` → per-drawing memory cap.
- A tiny runtime per drawing; 8 of them are cheap.
- Speed is not a concern here: a frame is ~100–1,000 simple calls.

Setup per drawing:
1. Create the runtime and context. Set the memory limit and interrupt handler (checks a deadline set before each `frame()` call).
2. Remove or don't install anything beyond the ES built-ins: no `std`/`os` modules, no module loader.
3. Install globals: `peek` (`frame`, `on`, `log`), `Path2D`, and the internal recorder functions used by `ctx`.
4. Evaluate the script (top-level code runs once). Keep a reference to the `frame` callback.

### The `ctx` object

`ctx` is a **plain JS object implemented in JS** (a small prelude peek loads before the drawing). It keeps canvas state (`fillStyle`, `lineWidth`, `font`, …) as JS properties, and each call appends to **one shared buffer**:

- a `Float64Array` op stream: `[opcode, argc, args…]`
- a string table for colors, fonts and text
- a handle table for `Path2D` and `ImageHandle`

Setting a style property only records an op when its value actually changes. At the end of `frame()`, **one** native call passes `(buffer, length, strings, again)` to Swift. **Don't bridge each `ctx` call into Swift individually.** Crossing between JS and Swift once per call is where the cost would be.

`ctx.fillGlass`, `ctx.fillBlur` and changes to `ctx.vibrant` record **layer-break ops**. For glass and blur, the recorder in the prelude **applies the current transform to the path** and stores the flattened path, because glass layers have no transform context.

## B4. Frame loop

```
main thread (display link tick for slot S)
  1. if S is sleeping and no wake reason → nothing to do
  2. present frame N's layers (already prepared)
  3. build Input snapshot N+1 from InputHub, post to S's JS thread

S's JS thread
  4. deadline = now + 4ms; call frame(ctx, input) under the interrupt handler
  5. on success → hand buffer N+1 back to main; on throw/timeout → drop, count failure

main thread
  6. Compositor.prepare(buffer N+1)   // split + diff + render changed layers
```

- Use `NSView.displayLink(target:selector:)` (macOS 14+) on the `VisualView`. It follows the display's refresh rate (including 120 Hz ProMotion) and pauses when the view is offscreen.
- **Keep one frame in flight at most.** If the JS thread hasn't returned, skip, and don't queue work.
- **Sleep:** when `again == false`, stop the display link. Restart it for any wake reason listed in A3. `InputHub` publishes those changes, and the `DrawingHost` subscribes.
- `input.dt` = time since the last *executed* frame, capped at 0.1 s, and 0 on the first frame after waking.

## B5. Compositor: split → diff → render

**Split.** Walk the op buffer. Every layer-break op ends the current `draw` layer and emits a `glass`, `blur` or `vibrant-draw` layer. The result is an ordered list of layers.

**Diff.** Hash each layer (op bytes + referenced strings/handles). Compare with the same position in the previous frame:
- same kind + same hash → **reuse** (don't touch the view or layer)
- same kind, different hash → **update**
- structure changed (count or kinds differ) → rebuild the layer stack for this frame. This should be rare.

**Render.**

| Layer kind | Backing | Render |
|---|---|---|
| `draw` | `CALayer` with a bitmap | replay ops into a `CGContext` (B6) → `layer.contents` |
| `vibrant-draw` | `CALayer` inside an `NSVisualEffectView` with vibrancy | same replay, drawn in monochrome so vibrancy can color it |
| `glass` | SwiftUI `Color.clear.glassEffect(style.tint(tint).interactive(i), in: PathShape(path))` hosted in an `NSHostingView` | update the `PathShape` / options only when the hash changes |
| `blur` | `NSVisualEffectView(material:)` with `maskImage` or a layer mask from the path | update the mask only when the hash changes |

All layer views sit inside `VisualView` in order, and `VisualView` clips to its 100 × 100 bounds. Put multiple glass layers inside one `GlassEffectContainer` so they blend with each other the way system glass does.

**Even-odd glass:** if `glassEffect(in:)` ignores even-odd filling, convert the path first: `cgPath.normalized(using: .evenOdd)` (a CGPath boolean operation, macOS 14+) turns it into an equivalent path with holes that works under the default rule.

## B6. Replaying ops into CGContext

Set up once per `draw` layer bitmap:

```swift
let px = visualSizeInPoints * backingScale          // e.g. 120pt × 2 = 240px
let ctx = CGContext(data: nil, width: px, height: px, bitsPerComponent: 8, bytesPerRow: 0,
                    space: CGColorSpace(name: CGColorSpace.displayP3)!,
                    bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue)!
ctx.translateBy(x: 0, y: CGFloat(px)); ctx.scaleBy(x: 1, y: -1)   // y-down like canvas
ctx.scaleBy(x: CGFloat(px) / 100, y: CGFloat(px) / 100)           // 100 units → pixels
```

Op mapping:

| op | CGContext |
|---|---|
| `save` / `restore` | `saveGState()` / `restoreGState()` (and restore peek's own style state) |
| `translate` / `rotate` / `scale` / `transform` | `translateBy` / `rotate(by:)` / `scaleBy` / `concatenate(CGAffineTransform)` |
| `beginPath` | start a new `CGMutablePath` (keep the path on the Swift side, not in the context) |
| `moveTo` / `lineTo` / `arc` / `arcTo` / `ellipse` / curves / `rect` / `roundRect` | the matching `CGMutablePath` add calls, **with the current transform applied** |
| `fill(rule)` | `addPath` + `fillPath(using: .winding / .evenOdd)` with the current fill style |
| `stroke` | `addPath` + `setLineWidth/Cap/Join/Dash` + `strokePath()` |
| `clip(rule)` | `addPath` + `clip(using:)` |
| gradient fill | `saveGState`, clip to path, `drawLinearGradient` / `drawRadialGradient` / conic via `CGShading`, `restoreGState` |
| `globalAlpha` / `globalCompositeOperation` | `setAlpha` / `setBlendMode` |
| `shadow*` | `setShadow(offset:blur:color:)` |
| `filter blur(n)` | render the op range into an offscreen bitmap, apply `CIGaussianBlur`, composite back |
| `fillText` / `strokeText` | Core Text: build a `CTLine` from the font + text, handle `textAlign`/`textBaseline`, flip locally, `CTLineDraw` |
| `drawImage(handle, …)` | `draw(cgImage, in:)`; images are decoded once per send and cached by handle |

Canvas keeps the current path independent of transforms: points are transformed as they're added. Keep the path in Swift with that behavior, rather than relying on CGContext's own current path.

## B7. Images

- When a `send` arrives, peek **copies** each image file into its own cache right away (the silicon's file may be temporary), decodes it to a `CGImage` sized for the bubble (max 512 px), and computes `colors.dominant` + a 3–5 color palette (k-means on a 32 × 32 downsample).
- The JS side only gets an opaque handle id plus `width`/`height`. Handles become invalid when the next `send` replaces the content, and drawing an invalid handle does nothing.

## B8. InputHub: where each input comes from

| Input | Source |
|---|---|
| `t`, `dt` | frame scheduler |
| `slot`, `mode` | SlotManager / settings. `facing` = `atan2` from the slot center to the screen center, in y-down coordinates |
| `appearance` | `NSApp.effectiveAppearance`, observed for changes |
| `backdrop` | see below |
| `phase` | SlotManager's state machine |
| `hover`, `mouse` | `NSEvent.mouseLocation` each frame while visible, plus `NSEvent.addGlobalMonitorForEvents(matching: .mouseMoved)` / a local monitor to wake sleeping drawings. Converted to the slot's 100 × 100 units |
| `speech.level` | TTS audio: render speech with `AVSpeechSynthesizer.write(_:toBufferCallback:)`, play the buffers through `AVAudioEngine`, and compute RMS per buffer (smoothed, 0…1) |
| `speech.word`, `progress` | `AVSpeechSynthesizerDelegate.speechSynthesizer(_:willSpeakRangeOfSpeechString:utterance:)` → also fires the `word` event |
| `mic.level` | `AVAudioEngine.inputNode` tap → RMS → smoothed 0…1 (the same data drives peek's own live waveform) |
| `mic.transcript` | on-device speech recognition (`SpeechAnalyzer` on macOS 26+, `SFSpeechRecognizer` with `requiresOnDeviceRecognition` as a fallback) |
| `typing` | the typing field's text binding |
| `show`, `ask` | the current `send` payload, with image paths replaced by handles; `ask.value` / `highlight` update live from the arc controls and voice matching |

### Backdrop tracking

Peek needs to know what each bubble sits over, both for its own arc and pills (white or black shading, per `understanding.md`) and for `input.backdrop`. In order of preference:

1. **`screen`**: if the user has granted Screen Recording permission, use ScreenCaptureKit to capture the region under each visible bubble **excluding peek's own windows**, at about 2 Hz and a tiny resolution (16 × 16). Average the color and compute luminance.
2. **`wallpaper`**: no permission needed. Read the current desktop picture (`NSWorkspace.shared.desktopImageURL(for:)`), sample the region under the bubble, and cache it per screen until the wallpaper changes.
3. **`appearance`**: last resort. Treat dark mode as a dark backdrop and light mode as a light one.

`tone = luminance < 0.5 ? 'dark' : 'light'`. `ink` is the opposite of `tone`. Change `tone` with **hysteresis** (switch at 0.45 / 0.55) so it doesn't flicker over mid-gray content.

## B9. Clicks and hit-testing

- `isOverContent(point)` = the point is inside any glass/blur path of the current frame, **or** the pixel under it in any draw layer has alpha > 0.05, **or** it's over peek's own chrome (arc, pills, buttons).
- A click on the visual → convert to units → `click` event `{ x, y, count }` → wake the drawing. Nothing else happens (per `understanding.md`).

## B10. Failure handling

| Failure | Action |
|---|---|
| `frame()` throws | drop the frame, keep showing the last one, log with the stack trace |
| over the 4 ms deadline | interrupt handler aborts, drop the frame |
| 10 throws in a row, or 30 overruns in 5 s | switch to the **fallback visual**, record the error for the silicon's next `peek` CLI call |
| memory limit hit | destroy the runtime → fallback visual |
| unknown op or bad args | ignore that op, log once |

The fallback visual is drawn by peek natively: a glass circle with the silicon's initial in SF Pro Rounded.

## B11. Registration, storage, simulation

- `peek register drawing` runs validation (A9) in a **separate, temporary** runtime, rendering offscreen into a bitmap (the glass layers are drawn as a flat translucent approximation for `--preview`).
- On success, store the script at `~/Library/Application Support/peek/drawings/<silicon-id>/<sha256>.js`, upload it to the server (per `understanding.md`), and swap it into the slot's `DrawingHost`.
- **Simulation mode** uses the same `DrawingHost` with a fake `InputHub` fed by the simulation toggles (position, speak, show, ask, mode, appearance, backdrop). `peek.log` output is shown in the simulation panel.

## B12. Verify these first (one-day spike)

Before building everything, build one bubble that proves the risky parts:

1. **Glass with holes:** does `glassEffect(in:)` accept an even-odd path, or does it need `normalized(using:)`? Do the holes look right?
2. **Changing glass every frame:** how slow is it, and how bad does it look? This sets how hard the A6.1 rule and the validation warning should be.
3. **Glass over draw layers:** does content drawn before a glass layer (in the same window) actually refract through it?
4. **Click-through:** does toggling `ignoresMouseEvents` from pointer tracking feel instant, including clicks through the cassette's reel holes?
5. **Frame cost:** 8 bubbles awake at 120 Hz, each drawing the cassette. Measure CPU. Target < 5% total on Apple Silicon.
6. **TTS level + word timing:** does `write(_:toBufferCallback:)` + `AVAudioEngine` playback keep word callbacks in sync with the audio?
