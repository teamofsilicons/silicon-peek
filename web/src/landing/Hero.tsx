import { For, Show, createMemo, createSignal, onCleanup, onMount, type JSX } from "solid-js";
import { copyText } from "../shared/copy.ts";
import { HOTKEY, INSTALL_COMMAND, docsHref } from "../shared/site.ts";
import { track } from "../shared/telemetry.ts";
import { CoverArt, DotDrawing, EyeDrawing, GaugeDrawing, MonogramDrawing, OrbDrawing, SunDrawing, VinylDrawing } from "./Art.tsx";
import {
  SLOTS,
  arcFraction,
  arcPath,
  arcPlacement,
  arcPoint,
  arcSpanPath,
  curvedBand,
  snapToStep,
  type SlotGeometry,
} from "./geometry.ts";
import Wallpaper from "./Wallpaper.tsx";

// ---------------------------------------------------------------------------------------------
// Demo model. Phases and wording follow the real product (docs/drawing.md `input.phase`,
// docs/ask.md voice rules, docs/carbon.md bubble behaviour), so the hero is a faithful, if
// simulated, peek.
// ---------------------------------------------------------------------------------------------

type Mode = "show" | "speak" | "ask" | "slider";
type Phase = "showing" | "speaking" | "asking" | "listening" | "typing" | "transcribing" | "leaving";
type Answer = { label: string; via: "click" | "keyboard" | "voice" };

const MODES: readonly Mode[] = ["show", "speak", "ask", "slider"];
const MODE_LABEL: Record<Mode, string> = { show: "Show", speak: "Speak", ask: "Ask", slider: "Slider" };
/** What the automatic tour plays after each mode. */
const NEXT: Record<Mode, Mode> = { show: "speak", speak: "ask", ask: "slider", slider: "show" };
const QUESTION = "Play the B-side next?";
const OPTIONS = ["Play it", "Skip"] as const;
const CAPTION_TEXT = "Low Tide by The Silicons";
const NOTE = "Recorded in one take on the pier at dusk. The B-side is mostly the rain.";
const SLIDER = { question: "Crossfade into the B-side?", min: 0, max: 10, step: 2, initial: 4, unit: "s" } as const;
const COMMANDS: Record<Mode, string> = {
  show: `peek send --show '{"elements":[{"type":"text","text":"2022"},{"type":"image","path":"./covers/low-tide.png","caption":"${CAPTION_TEXT}"},{"type":"text","text":"${NOTE}"}]}'`,
  speak: `peek send --speak "Up next: Low Tide, from 2022."`,
  ask: `peek send --speak "Want the B-side next?" --ask '{"question":"${QUESTION}","type":"single_choice","options":["${OPTIONS[0]}","${OPTIONS[1]}"]}'`,
  slider: `peek send --speak "How long a crossfade?" --ask '{"question":"${SLIDER.question}","type":"slider","min":${SLIDER.min},"max":${SLIDER.max},"step":${SLIDER.step},"default":${SLIDER.initial},"unit":"${SLIDER.unit}"}'`,
};
const NO_MATCH = "Didn't match an option — tap one or type";

/** Matches typed or transcribed text to an option the way peek does: labels, then ordinals. */
export function matchOption(text: string): string | null {
  const t = text.trim().toLowerCase().replace(/[.!?]+$/, "");
  if (!t) return null;
  const exact = OPTIONS.find((o) => o.toLowerCase() === t || t.includes(o.toLowerCase()));
  if (exact) return exact;
  if (/^(1|one|first|the first( one)?)$/.test(t)) return OPTIONS[0];
  if (/^(2|two|second|the second( one)?)$/.test(t)) return OPTIONS[1];
  if (/^(yes|yeah|sure|play)$/.test(t)) return OPTIONS[0];
  if (/^(no|nope|next)$/.test(t)) return OPTIONS[1];
  return null;
}

const NUMBER_WORDS = ["zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten"];

/** A typed number for the slider ask ("6", "six seconds"), snapped to the step like peek does. */
export function matchNumber(text: string): number | null {
  const t = text.trim().toLowerCase();
  const digits = /-?\d+(?:\.\d+)?/.exec(t);
  let n: number | null = digits ? Number(digits[0]) : null;
  if (n === null) {
    const i = NUMBER_WORDS.findIndex((w) => new RegExp(`\\b${w}\\b`).test(t));
    n = i >= 0 ? i : null;
  }
  return n === null || !Number.isFinite(n) ? null : snapToStep(n, SLIDER.min, SLIDER.max, SLIDER.step);
}

/** A speech-like loudness curve in 0…1: syllables at ~4 Hz inside slower phrases, with pauses. */
function syntheticLevel(t: number): number {
  const syllable = Math.max(0, Math.sin(t * Math.PI * 2 * 4.2)) ** 0.7;
  const phrase = 0.62 + 0.38 * Math.sin(t * 1.9 + 0.6);
  const pause = Math.sin(t * 2.6) > -0.82 ? 1 : 0.08;
  const grain = 0.12 * Math.sin(t * 23.3 + Math.sin(t * 3.1) * 2);
  return Math.min(1, Math.max(0, (syllable * phrase + grain * syllable) * pause));
}

const clamp = (n: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, n));

// ---------------------------------------------------------------------------------------------
// Idle positions
// ---------------------------------------------------------------------------------------------

type Resident = { name: string; drawing: (pupil: (el: SVGCircleElement) => void) => JSX.Element };

const RESIDENTS: Record<number, Resident> = {
  1: { name: "si:standup", drawing: () => <OrbDrawing /> },
  2: { name: "si:watch", drawing: (pupil) => <EyeDrawing pupil={pupil} /> },
  3: { name: "si:cleanup", drawing: () => <DotDrawing /> },
  4: { name: "si:vinyl", drawing: () => <VinylDrawing /> },
  6: { name: "si:budget", drawing: () => <GaugeDrawing /> },
  7: { name: "si:remind", drawing: () => <MonogramDrawing letter="R" /> },
  8: { name: "si:weather", drawing: () => <SunDrawing /> },
};

// Idle arcs, in visual diameters: centre 1.8 behind the visual, radius 2.55, ±34°.
const IDLE_ARC = { pivot: 1.8, radius: 2.55, spread: 0.6 };

function IdleSlot(props: { slot: SlotGeometry; resident: Resident; pupil: (el: SVGCircleElement) => void }) {
  const [bump, setBump] = createSignal(false);
  let timer: number | undefined;
  onCleanup(() => clearTimeout(timer));
  const d = arcPath(props.slot.facing, IDLE_ARC.radius, IDLE_ARC.spread, IDLE_ARC.pivot);
  // Push the hotkey pill past the arc, further on the sides where the pill is wide along the facing.
  const extra = 0.2 + 0.42 * Math.abs(Math.cos(props.slot.facing));
  const pill = arcPlacement(props.slot.facing, IDLE_ARC.radius + extra, 0, IDLE_ARC.pivot);
  return (
    <div class="slot idle" data-index={props.slot.index} data-side={props.slot.side}>
      <svg class="arc-guide" viewBox="-3.2 -3.2 6.4 6.4" aria-hidden="true">
        <path d={d} />
      </svg>
      <button
        type="button"
        class="visual glass circle"
        classList={{ bump: bump() }}
        aria-label={`Position ${props.slot.index}, ${props.slot.side}: ${props.resident.name}, shortcut ${HOTKEY.text}+${props.slot.index}. Clicking a visual only animates it.`}
        onClick={() => {
          setBump(false);
          requestAnimationFrame(() => setBump(true));
          clearTimeout(timer);
          timer = window.setTimeout(() => setBump(false), 420);
        }}
      >
        {props.resident.drawing(props.pupil)}
      </button>
      <span class="hotkey-pill" style={{ "--x": pill.x, "--y": pill.y }} aria-hidden="true">
        <kbd>
          {HOTKEY.glyphs}
          {props.slot.index}
        </kbd>{" "}
        {props.resident.name.slice(3)}
      </span>
    </div>
  );
}

// ---------------------------------------------------------------------------------------------
// The featured bubble: position 5, the DJ Silicon, reproducing the reference screenshot
// ---------------------------------------------------------------------------------------------

// Arc geometry in visual diameters (v5). Show: the caption sits on the arc's middle, the year pill
// to its left and the long note to its right. Ask: options (or the slider track) sit on a slightly
// higher arc with the question curving above; the answer field is a curved band right under it.
const SHOW_ARC = { pivot: 2.06, radius: 2.87 };
const ASK_ARC = { pivot: 1.9, radius: 2.87 };
const UP = -Math.PI / 2;
const rad = (deg: number) => (deg * Math.PI) / 180;
const YEAR = arcPlacement(UP, SHOW_ARC.radius, rad(-21), SHOW_ARC.pivot);
const CAPTION = arcPlacement(UP, SHOW_ARC.radius, 0, SHOW_ARC.pivot);
// Long text becomes a narrow, tall column that grows away from the screen edge (upward here). It
// leans half as much as a one-line pill would, which keeps several lines easy to read.
const NOTE_PLACE = arcPlacement(UP, SHOW_ARC.radius, rad(25), SHOW_ARC.pivot);
const NOTE_AT = { ...NOTE_PLACE, rotate: NOTE_PLACE.rotate / 2 };
const OPTION_AT = [-0.2, 0.2].map((o) => arcPlacement(UP, ASK_ARC.radius, o, ASK_ARC.pivot));
const NOTICE_AT = arcPlacement(UP, ASK_ARC.radius - 0.17, 0, ASK_ARC.pivot); // under the band
const QUESTION_PATH = arcPath(UP, ASK_ARC.radius + 0.6, 0.42, ASK_ARC.pivot);

// The slider track takes the arc; the ✓ that sends the value sits at its end.
const TRACK = { from: -0.34, to: 0.24 };
const trackOffset = (f: number) => TRACK.from + f * (TRACK.to - TRACK.from);
const TRACK_PATH = arcSpanPath(UP, ASK_ARC.radius, TRACK.from, TRACK.to, ASK_ARC.pivot);
const CONFIRM_AT = { ...arcPlacement(UP, ASK_ARC.radius, 0.395, ASK_ARC.pivot), rotate: 0 }; // an icon stays upright
const STEPS = Array.from({ length: Math.round((SLIDER.max - SLIDER.min) / SLIDER.step) + 1 }, (_, i) => SLIDER.min + i * SLIDER.step);
/** Dots mark the steps when there is a sensible number of them. */
const STEP_DOTS = STEPS.length <= 21 ? STEPS : [];
const fracOf = (v: number) => (v - SLIDER.min) / (SLIDER.max - SLIDER.min);
const valueOf = (f: number) => snapToStep(SLIDER.min + clamp(f, 0, 1) * (SLIDER.max - SLIDER.min), SLIDER.min, SLIDER.max, SLIDER.step);
const withUnit = (v: number) => `${v} ${SLIDER.unit}`;
const SCALE_LIFT = 0.27; // value pill and bounds ride this far outside the track

// The curved answer band (waveform, transcribing, typing field), in the same arc family.
const BAND = curvedBand(UP, ASK_ARC.pivot, 3.1, 3.38, 0.36);
const BAND_VIEWBOX = `${BAND.box.x} ${BAND.box.y} ${BAND.box.w} ${BAND.box.h}`;
const BAND_TEXT_PATH = arcPath(UP, 3.198, 0.36, ASK_ARC.pivot);
const BAND_MASK = `url("data:image/svg+xml,${encodeURIComponent(
  `<svg xmlns='http://www.w3.org/2000/svg' viewBox='${BAND_VIEWBOX}' preserveAspectRatio='none'><path d='${BAND.d}'/></svg>`,
)}")`;
const BAR_COUNT = 26;
const BARS = Array.from({ length: BAR_COUNT }, (_, i) => {
  const offset = -0.31 + (0.62 * i) / (BAR_COUNT - 1);
  const c = arcPoint(UP, 3.24, offset, ASK_ARC.pivot);
  return { x: c.x, y: c.y, ux: Math.cos(UP + offset), uy: Math.sin(UP + offset) };
});
/** How many characters fit along the band before the start is elided. */
const BAND_CHARS = 26;

const pos = (p: { x: number; y: number; rotate?: number }) => ({ "--x": p.x, "--y": p.y, "--r": `${p.rotate ?? 0}deg` });

// ---------------------------------------------------------------------------------------------
// Reveal: cut-off text shows in full on hover; long text opens in place when clicked
// ---------------------------------------------------------------------------------------------

type Tip = { text: string; x: number; y: number; below: boolean; mono: boolean; ready: boolean };
type Pop = { text: string; left: number; y: number; fromBottom: boolean; width: number; origin: string; closing: boolean; source: HTMLElement };

const isCut = (el: HTMLElement) => el.scrollWidth > el.clientWidth + 1 || el.scrollHeight > el.clientHeight + 1;

export default function Hero() {
  const reduceMotion = typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;

  const [mode, setMode] = createSignal<Mode>("show");
  const [phase, setPhase] = createSignal<Phase>("showing");
  const [out, setOut] = createSignal(false);
  const [answer, setAnswer] = createSignal<Answer | null>(null);
  const [notice, setNotice] = createSignal<string | null>(null);
  const [typed, setTyped] = createSignal("");
  const [focused, setFocused] = createSignal(false);
  const [highlight, setHighlight] = createSignal<string | null>(null);
  const [autoplay, setAutoplay] = createSignal(!reduceMotion);
  const [copied, setCopied] = createSignal<"idle" | "copied" | "manual">("idle");
  const [captionCut, setCaptionCut] = createSignal(false);
  const [noteCut, setNoteCut] = createSignal(false);
  const [frac, setFrac] = createSignal(fracOf(SLIDER.initial));
  const [dragging, setDragging] = createSignal(false);
  const [confirmPress, setConfirmPress] = createSignal(false);
  const [tip, setTip] = createSignal<Tip | null>(null);
  const [pop, setPop] = createSignal<Pop | null>(null);

  let timers: number[] = [];
  const later = (ms: number, fn: () => void) => timers.push(window.setTimeout(fn, ms));
  /** A step of the automatic tour: skipped once the visitor has taken over. */
  const auto = (ms: number, fn: () => void) => later(ms, () => autoplay() && fn());
  const clearTimers = () => {
    timers.forEach(clearTimeout);
    timers = [];
  };

  let hero!: HTMLElement;
  let input: HTMLInputElement | undefined;
  let reelLeft: SVGGElement | undefined;
  let reelRight: SVGGElement | undefined;
  let stage: HTMLDivElement | undefined;
  let bubble: HTMLDivElement | undefined;
  let visualEl: HTMLDivElement | undefined;
  let pupil: SVGCircleElement | undefined;
  let tipEl: HTMLDivElement | undefined;
  let popEl: HTMLDivElement | undefined;
  const bars: SVGLineElement[] = [];
  const history: number[] = Array(BAR_COUNT).fill(0);

  // ------------------------------------------------------------------ the slider's motion
  let springRaf = 0;
  let simRaf = 0;
  let velocity = 0;
  const stopMotion = () => {
    cancelAnimationFrame(springRaf);
    cancelAnimationFrame(simRaf);
    springRaf = simRaf = 0;
  };

  /** Springs the thumb to `target` (a fraction), carrying the drag's velocity, with a little bounce. */
  function springTo(target: number) {
    cancelAnimationFrame(springRaf);
    if (reduceMotion) {
      setFrac(target);
      return;
    }
    let x = frac();
    let v = clamp(velocity, -6, 6);
    let last = performance.now();
    const step = (now: number) => {
      const dt = Math.min(0.032, (now - last) / 1000);
      last = now;
      v += (-420 * (x - target) - 21 * v) * dt;
      x += v * dt;
      if (Math.abs(x - target) < 0.0006 && Math.abs(v) < 0.01) {
        setFrac(target);
        springRaf = 0;
        return;
      }
      setFrac(clamp(x, -0.03, 1.03));
      springRaf = requestAnimationFrame(step);
    };
    springRaf = requestAnimationFrame(step);
  }

  const snapValue = () => valueOf(frac());

  /** The fraction along the track under a pointer, following it continuously (no per-step jumps). */
  function pointerFraction(e: PointerEvent): number {
    const o = bubble!.getBoundingClientRect();
    const v = visualEl!.getBoundingClientRect().width || 1;
    return arcFraction(UP, ASK_ARC.pivot, TRACK.from, TRACK.to, { x: (e.clientX - o.left) / v, y: (e.clientY - o.top) / v });
  }

  let lastMove = { f: 0, t: 0 };
  function dragStart(e: PointerEvent) {
    if (!(mode() === "slider" && phase() === "asking")) return;
    e.preventDefault();
    takeOver();
    stopMotion();
    (e.currentTarget as Element).setPointerCapture(e.pointerId);
    setDragging(true);
    velocity = 0;
    const f = pointerFraction(e);
    lastMove = { f, t: performance.now() };
    setFrac(f);
  }
  function dragMove(e: PointerEvent) {
    if (!dragging()) return;
    const f = pointerFraction(e);
    const now = performance.now();
    const dt = Math.max(0.008, (now - lastMove.t) / 1000);
    velocity = velocity * 0.6 + ((f - lastMove.f) / dt) * 0.4;
    lastMove = { f, t: now };
    setFrac(f);
  }
  function dragEnd() {
    if (!dragging()) return;
    setDragging(false);
    if (performance.now() - lastMove.t > 90) velocity = 0;
    springTo(fracOf(snapValue())); // snap to the nearest step, with a spring
  }

  function nudge(steps: number) {
    velocity = 0;
    springTo(fracOf(snapToStep(snapValue() + steps * SLIDER.step, SLIDER.min, SLIDER.max, SLIDER.step)));
  }

  /** The tour's drag: the thumb glides with the "pointer", then springs to the nearest step. */
  function simulateDrag() {
    const start = frac();
    const end = 0.73;
    const began = performance.now();
    setDragging(true);
    const step = (now: number) => {
      const t = Math.min(1, (now - began) / 1150);
      const ease = t < 0.5 ? 4 * t * t * t : 1 - (-2 * t + 2) ** 3 / 2;
      setFrac(start + (end - start) * ease + Math.sin(t * Math.PI) * 0.015);
      if (t < 1) simRaf = requestAnimationFrame(step);
      else {
        simRaf = 0;
        setDragging(false);
        velocity = 0.5;
        springTo(fracOf(snapValue()));
        auto(1300, () => {
          setConfirmPress(true);
          later(200, () => {
            setConfirmPress(false);
            deliver({ label: withUnit(snapValue()), via: "click" });
          });
        });
      }
    };
    simRaf = requestAnimationFrame(step);
  }

  // ------------------------------------------------------------------ the demo's flow
  function play(next: Mode) {
    clearTimers();
    stopMotion();
    closePop(true);
    hideTip();
    setMode(next);
    setAnswer(null);
    setNotice(null);
    setTyped("");
    setHighlight(null);
    setDragging(false);
    setConfirmPress(false);
    setFrac(fracOf(SLIDER.initial));
    setOut(false);
    if (next === "show") {
      setPhase("showing");
      auto(7000, () => play("speak"));
    } else if (next === "speak") {
      setPhase("speaking");
      later(4200, () => {
        setPhase("showing"); // speech.done: the bubble would slide back 1.5 s later
        auto(1500, () => play("ask"));
      });
    } else {
      setPhase("speaking");
      later(1900, () => {
        setPhase("asking");
        if (next === "ask") auto(3200, () => startListening());
        else auto(1300, () => simulateDrag());
      });
    }
  }

  function takeOver() {
    if (autoplay()) setAutoplay(false);
    cancelAnimationFrame(simRaf);
    simRaf = 0;
  }

  function deliver(a: Answer) {
    clearTimers();
    stopMotion();
    setAnswer(a);
    setPhase("leaving");
    later(450, () => setOut(true));
    later(3600, () => play(autoplay() ? NEXT[mode()] : mode()));
  }

  function startListening() {
    setNotice(null);
    setPhase("listening");
    auto(2300, () => stopListening());
  }

  function stopListening() {
    setPhase("transcribing");
    // The real app uploads the finished WAV once; here the "transcript" is simulated.
    later(1100, () => deliver(mode() === "slider" ? { label: withUnit(8), via: "voice" } : { label: OPTIONS[0], via: "voice" }));
  }

  function startTyping() {
    takeOver();
    setNotice(null);
    setPhase("typing");
    queueMicrotask(() => input?.focus({ preventScroll: true }));
  }

  function submitTyped() {
    if (mode() === "slider") {
      const n = matchNumber(typed());
      if (n !== null) deliver({ label: withUnit(n), via: "keyboard" });
      else setNotice(NO_MATCH);
      return;
    }
    const match = matchOption(typed());
    if (match) deliver({ label: match, via: "keyboard" });
    else setNotice(NO_MATCH);
  }

  function cancelInput() {
    setNotice(null);
    setTyped("");
    setPhase("asking");
  }

  let arrowTimer: number | undefined;
  function downArrow(e: MouseEvent) {
    takeOver();
    clearTimeout(arrowTimer);
    const stopAudio = e.detail >= 2;
    const slide = () => {
      clearTimers();
      stopMotion();
      closePop(true);
      if (stopAudio && phase() === "speaking") setPhase("showing");
      setOut(true);
      later(2600, () => play(mode()));
    };
    if (stopAudio) slide();
    else arrowTimer = window.setTimeout(slide, 260);
  }

  // ------------------------------------------------------------------ reveal: tooltip and popup
  let tipTimer: number | undefined;
  function hideTip() {
    clearTimeout(tipTimer);
    setTip(null);
  }
  /** Shows the complete text of a cut-off element in a glass tooltip, placed toward the screen centre. */
  function showTip(el: HTMLElement, text: string, mono = false, cutEl: HTMLElement = el) {
    clearTimeout(tipTimer);
    tipTimer = window.setTimeout(() => {
      if (pop() || !isCut(cutEl)) return;
      const h = hero.getBoundingClientRect();
      const r = el.getBoundingClientRect();
      const below = (r.top + r.bottom) / 2 < h.top + h.height / 2;
      setTip({ text, x: r.left + r.width / 2 - h.left, y: below ? r.bottom - h.top + 8 : r.top - h.top - 8, below, mono, ready: false });
      requestAnimationFrame(() => {
        const t = tip();
        if (!t || !tipEl) return;
        const w = tipEl.offsetWidth;
        const th = tipEl.offsetHeight;
        let { below: under, y } = t;
        if (!under && y - th < 6) [under, y] = [true, r.bottom - h.top + 8];
        else if (under && y + th > h.height - 6) [under, y] = [false, r.top - h.top - 8];
        setTip({ ...t, x: clamp(t.x, w / 2 + 10, h.width - w / 2 - 10), y, below: under, ready: true });
      });
    }, 70);
  }
  /** Hover and focus handlers for an element whose text may be cut off (in `cutEl`, or itself). */
  const reveal = (text: () => string, mono = false, cutEl?: () => HTMLElement) => ({
    onPointerEnter: (e: PointerEvent) => showTip(e.currentTarget as HTMLElement, text(), mono, cutEl?.()),
    onPointerLeave: hideTip,
    onFocus: (e: FocusEvent) => showTip(e.currentTarget as HTMLElement, text(), mono, cutEl?.()),
    onBlur: hideTip,
  });

  /** Opens long text in a small glass popup right where it is, growing away from the nearer edge. */
  function togglePop(el: HTMLElement, text: string, cutEl: HTMLElement) {
    const open = pop();
    if (open && !open.closing && open.source === el) {
      closePop();
      return;
    }
    if (!isCut(cutEl)) return;
    clearTimeout(popTimer);
    hideTip();
    const h = hero.getBoundingClientRect();
    const r = el.getBoundingClientRect();
    const width = Math.min(260, h.width - 24);
    const cx = r.left + r.width / 2 - h.left;
    const left = clamp(cx - width / 2, 12, h.width - width - 12);
    const fromBottom = (r.top + r.bottom) / 2 > h.top + h.height / 2;
    setPop({
      text,
      left,
      width,
      fromBottom,
      y: Math.max(8, fromBottom ? h.bottom - r.bottom - 4 : r.top - h.top - 4),
      origin: `${Math.round(cx - left)}px ${fromBottom ? `calc(100% - ${Math.round(r.height / 2 + 4)}px)` : `${Math.round(r.height / 2 + 4)}px`}`,
      closing: false,
      source: el,
    });
  }
  let popTimer: number | undefined;
  function closePop(now = false) {
    const p = pop();
    if (!p) return;
    clearTimeout(popTimer);
    if (now || reduceMotion) {
      setPop(null);
      return;
    }
    setPop({ ...p, closing: true });
    popTimer = window.setTimeout(() => setPop(null), 220);
  }

  const announce = createMemo(() => {
    const a = answer();
    if (a) return `Answer delivered to si:dj as peek.ask.answered: ${a.label}, via ${a.via}.`;
    if (out()) return "The bubble slid back.";
    switch (phase()) {
      case "showing":
        return mode() === "show" ? `Showing: ${CAPTION_TEXT}, 2022. ${NOTE}` : "Speech finished.";
      case "speaking":
        return "Speaking.";
      case "asking":
        return mode() === "slider"
          ? `Asking: ${SLIDER.question} A slider from ${withUnit(SLIDER.min)} to ${withUnit(SLIDER.max)} in steps of ${SLIDER.step}.`
          : `Asking: ${QUESTION} Options: ${OPTIONS.join(", ")}.`;
      case "listening":
        return "Listening. Click the waveform to stop.";
      case "typing":
        return "Typing an answer. Press Return to send, Escape to throw it away.";
      case "transcribing":
        return "Transcribing.";
      default:
        return "";
    }
  });

  // One animation loop for the cassette reels, the level orb, the mic waveform and the eye.
  let pointer: { x: number; y: number } | null = null;
  let raf = 0;
  let visible = true;
  onMount(() => {
    const started = performance.now();
    let last = started;
    let spin = 0;
    let level = 0;
    let lookX = 0;
    let lookY = 0;
    const tick = (now: number) => {
      raf = 0;
      const dt = Math.min(0.1, (now - last) / 1000);
      last = now;
      const t = (now - started) / 1000;
      const p = phase();
      const voiced = p === "speaking" || p === "listening";
      const target = voiced && !reduceMotion ? syntheticLevel(t) : 0;
      level += (target - level) * Math.min(1, dt * 10);
      spin += (voiced && !reduceMotion ? 2 + level * 8 : 0) * dt;
      const deg = (spin * 180) / Math.PI;
      reelLeft?.setAttribute("transform", `translate(30 50) rotate(${deg.toFixed(2)})`);
      reelRight?.setAttribute("transform", `translate(70 50) rotate(${deg.toFixed(2)})`);
      bubble?.style.setProperty("--level", level.toFixed(3));
      if (p === "listening") {
        history.push(level);
        history.shift();
        bars.forEach((bar, i) => {
          const b = BARS[i];
          const half = 0.012 + Math.max(0.05, history[i]) * 0.1;
          bar.setAttribute("x1", (b.x - b.ux * half).toFixed(4));
          bar.setAttribute("y1", (b.y - b.uy * half).toFixed(4));
          bar.setAttribute("x2", (b.x + b.ux * half).toFixed(4));
          bar.setAttribute("y2", (b.y + b.uy * half).toFixed(4));
        });
      }
      if (pupil) {
        let ax = Math.cos((3 * Math.PI) / 4) * 6;
        let ay = Math.sin((3 * Math.PI) / 4) * 6;
        if (pointer) {
          const box = (pupil.ownerSVGElement as SVGSVGElement).getBoundingClientRect();
          const cx = box.left + box.width / 2;
          const cy = box.top + box.height / 2;
          const units = box.width / 100;
          const dx = pointer.x - cx;
          const dy = pointer.y - cy;
          const dist = Math.hypot(dx, dy) / units;
          const a = Math.atan2(dy, dx);
          const d = Math.min(10, dist / 4);
          ax = Math.cos(a) * d;
          ay = Math.sin(a) * d;
        }
        lookX += (ax - lookX) * Math.min(1, dt * 10);
        lookY += (ay - lookY) * Math.min(1, dt * 10);
        pupil.setAttribute("cx", (50 + lookX).toFixed(2));
        pupil.setAttribute("cy", (50 + lookY).toFixed(2));
      }
      if (visible && !document.hidden) raf = requestAnimationFrame(tick);
    };
    const resume = () => {
      if (!raf && visible && !document.hidden) {
        last = performance.now();
        raf = requestAnimationFrame(tick);
      }
    };
    const onPointer = (e: PointerEvent) => {
      pointer = { x: e.clientX, y: e.clientY };
    };
    const onVisibility = () => resume();
    const observer = new IntersectionObserver((entries) => {
      visible = entries.some((entry) => entry.isIntersecting);
      resume();
    });
    if (stage) observer.observe(stage);
    // The popup closes on a click outside it (a click on its own text is handled by togglePop) or Esc.
    const onDown = (e: PointerEvent) => {
      const p = pop();
      if (!p || p.closing) return;
      const target = e.target as Node;
      if (popEl?.contains(target) || p.source.contains(target)) return;
      closePop();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && pop()) {
        closePop();
        hideTip();
      }
    };
    const onResize = () => {
      closePop(true);
      hideTip();
    };
    window.addEventListener("pointermove", onPointer, { passive: true });
    document.addEventListener("pointerdown", onDown, true);
    document.addEventListener("keydown", onKey);
    window.addEventListener("resize", onResize);
    document.addEventListener("visibilitychange", onVisibility);
    resume();
    // ?demo=show|speak|ask|slider opens the demo in that state (and stops the automatic cycle).
    const requested = new URLSearchParams(location.search).get("demo");
    if (requested && (MODES as readonly string[]).includes(requested)) {
      setAutoplay(false);
      play(requested as Mode);
    } else play("show");
    onCleanup(() => {
      cancelAnimationFrame(raf);
      stopMotion();
      observer.disconnect();
      window.removeEventListener("pointermove", onPointer);
      document.removeEventListener("pointerdown", onDown, true);
      document.removeEventListener("keydown", onKey);
      window.removeEventListener("resize", onResize);
      document.removeEventListener("visibilitychange", onVisibility);
      clearTimers();
      clearTimeout(arrowTimer);
      clearTimeout(tipTimer);
      clearTimeout(popTimer);
    });
  });

  /** Keeps `set` in sync with whether `el` is cut off (its size follows the screen). */
  const watchCut = (set: (cut: boolean) => void) => (el: HTMLElement) => {
    const check = () => set(isCut(el));
    const ro = new ResizeObserver(check);
    ro.observe(el);
    onCleanup(() => ro.disconnect());
    requestAnimationFrame(check);
  };

  async function copyInstall(e: MouseEvent) {
    track("install_command_copied", { place: "hero" });
    const ok = await copyText(INSTALL_COMMAND, (e.currentTarget as HTMLElement).previousElementSibling ?? undefined);
    setCopied(ok ? "copied" : "manual");
    window.setTimeout(() => setCopied("idle"), 2200);
  }

  const inAsk = () => mode() === "ask" || mode() === "slider";
  const asking = () => ["asking", "listening", "typing", "transcribing"].includes(phase());
  const showOn = () => mode() === "show";
  const showLive = () => showOn() && !out() && phase() !== "leaving";
  const choiceOn = () => mode() === "ask" && (phase() === "asking" || phase() === "speaking");
  const sliderOn = () => mode() === "slider" && (phase() === "asking" || phase() === "speaking");
  const sliderLive = () => mode() === "slider" && phase() === "asking";
  const bandOn = () => ["listening", "typing", "transcribing"].includes(phase());
  const thumbAt = () => arcPlacement(UP, ASK_ARC.radius, trackOffset(frac()), ASK_ARC.pivot);
  const valueAt = () => arcPlacement(UP, ASK_ARC.radius + SCALE_LIFT, trackOffset(clamp(frac(), 0, 1)), ASK_ARC.pivot);
  const fillPath = () => arcSpanPath(UP, ASK_ARC.radius, TRACK.from, trackOffset(clamp(frac(), 0, 1)), ASK_ARC.pivot);
  const bandText = () => {
    const t = typed();
    return t.length > BAND_CHARS ? `…${t.slice(-(BAND_CHARS - 1))}` : t;
  };
  const placeholder = () => (mode() === "slider" ? "Type a number, like “6”…" : "Type “play it” or “skip”…");

  let captionText!: HTMLSpanElement;
  let noteText!: HTMLSpanElement;

  return (
    <section class="hero" id="top" aria-labelledby="hero-title" ref={hero}>
      <div class="desktop" ref={stage}>
        <Wallpaper />
        <div class="slots" role="group" aria-label="The eight peek positions around a Mac screen">
          <For each={SLOTS.filter((s) => s.index !== 5)}>
            {(slot) => <IdleSlot slot={slot} resident={RESIDENTS[slot.index]} pupil={(el) => (pupil = el)} />}
          </For>

          <div class="slot featured" data-index="5" data-side="bottom">
            {/* No panel and no background: only the visual, the arc and its elements, over the desktop. */}
            <div
              ref={bubble}
              class="bubble"
              classList={{ out: out(), [`mode-${mode()}`]: true, [`phase-${phase()}`]: true }}
              onPointerDown={takeOver}
              onKeyDown={(e) => {
                if (e.key === "Escape" && ["listening", "typing"].includes(phase())) {
                  e.preventDefault();
                  takeOver();
                  cancelInput();
                }
              }}
            >
              {/* information arc: show. The year is static; cut-off text is interactive. */}
              <span class="arc-item pill dark year" classList={{ on: showOn() }} style={pos(YEAR)}>
                2022
              </span>
              <div class="arc-item cover-group" classList={{ on: showOn() }} style={pos(CAPTION)}>
                <div class="cover-frame">
                  <CoverArt />
                </div>
                <button
                  type="button"
                  class="pill dark caption"
                  classList={{ tap: captionCut(), open: pop()?.text === CAPTION_TEXT && !pop()?.closing }}
                  tabIndex={showLive() && captionCut() ? 0 : -1}
                  aria-expanded={captionCut() ? pop()?.text === CAPTION_TEXT : undefined}
                  {...reveal(() => CAPTION_TEXT, false, () => captionText)}
                  onClick={(e) => {
                    takeOver();
                    togglePop(e.currentTarget, CAPTION_TEXT, captionText);
                  }}
                >
                  <span class="clip-line" ref={(el) => ((captionText = el), watchCut(setCaptionCut)(el))}>
                    {CAPTION_TEXT}
                  </span>
                </button>
              </div>
              <div class="arc-item note-group" classList={{ on: showOn() }} style={pos(NOTE_AT)}>
                <button
                  type="button"
                  class="pill dark note"
                  classList={{ tap: noteCut(), open: pop()?.text === NOTE && !pop()?.closing }}
                  tabIndex={showLive() && noteCut() ? 0 : -1}
                  aria-expanded={noteCut() ? pop()?.text === NOTE : undefined}
                  {...reveal(() => NOTE, false, () => noteText)}
                  onClick={(e) => {
                    takeOver();
                    togglePop(e.currentTarget, NOTE, noteText);
                  }}
                >
                  <span class="clip-lines" ref={(el) => ((noteText = el), watchCut(setNoteCut)(el))}>
                    {NOTE}
                  </span>
                </button>
              </div>

              {/* question arc, and the curved answer band right under it */}
              <svg class="ask-arcs" viewBox="-2.2 -2.2 4.4 2.4" aria-hidden="true">
                <path id="peek-question-path" d={QUESTION_PATH} fill="none" />
                <text class="question-text" classList={{ on: inAsk() && asking() }}>
                  <textPath href="#peek-question-path" startOffset="50%" text-anchor="middle">
                    {mode() === "slider" ? SLIDER.question : QUESTION}
                  </textPath>
                </text>
                {/* slider: the track follows the arc; dots mark the steps */}
                <g class="scale" classList={{ on: sliderOn() }}>
                  <path class="track-edge" d={TRACK_PATH} />
                  <path class="track" d={TRACK_PATH} />
                  <path class="track-fill" d={fillPath()} />
                  <For each={STEP_DOTS}>
                    {(v) => {
                      const p = arcPoint(UP, ASK_ARC.radius, trackOffset(fracOf(v)), ASK_ARC.pivot);
                      return (
                        <circle
                          class="step-dot"
                          classList={{ lit: v <= snapValue(), next: dragging() && v === snapValue() }}
                          cx={p.x}
                          cy={p.y}
                          r={0.024}
                        />
                      );
                    }}
                  </For>
                  <path
                    class="track-hit"
                    d={TRACK_PATH}
                    onPointerDown={dragStart}
                    onPointerMove={dragMove}
                    onPointerUp={dragEnd}
                    onPointerCancel={dragEnd}
                  />
                </g>
              </svg>

              {/* single choice */}
              <For each={OPTIONS}>
                {(label, i) => (
                  <div class="arc-item" classList={{ on: choiceOn() }} style={pos(OPTION_AT[i()])}>
                    <button
                      type="button"
                      class="pill glass option tap"
                      classList={{ hot: highlight() === label }}
                      tabIndex={mode() === "ask" && phase() === "asking" ? 0 : -1}
                      aria-hidden={!(mode() === "ask" && phase() === "asking")}
                      onPointerEnter={() => setHighlight(label)}
                      onPointerLeave={() => setHighlight(null)}
                      onFocus={() => setHighlight(label)}
                      onBlur={() => setHighlight(null)}
                      onClick={() => {
                        takeOver();
                        deliver({ label, via: "click" });
                      }}
                    >
                      {label}
                    </button>
                  </div>
                )}
              </For>

              {/* slider: bounds, the live value, the thumb and the ✓ */}
              <span class="arc-item bound" classList={{ on: sliderOn() && frac() > 0.2 }} style={pos(arcPlacement(UP, ASK_ARC.radius + SCALE_LIFT, TRACK.from, ASK_ARC.pivot))}>
                {withUnit(SLIDER.min)}
              </span>
              <span class="arc-item bound" classList={{ on: sliderOn() && frac() < 0.8 }} style={pos(arcPlacement(UP, ASK_ARC.radius + SCALE_LIFT, TRACK.to, ASK_ARC.pivot))}>
                {withUnit(SLIDER.max)}
              </span>
              <span class="arc-item pill glass value-pill" classList={{ on: sliderOn(), live: dragging() }} style={pos(valueAt())} aria-hidden="true">
                {withUnit(snapValue())}
              </span>
              <div class="arc-item thumb-anchor" classList={{ on: sliderOn() }} style={pos(thumbAt())}>
                <span
                  class="thumb glass tap"
                  classList={{ dragging: dragging() }}
                  role="slider"
                  tabIndex={sliderLive() ? 0 : -1}
                  aria-label={SLIDER.question}
                  aria-valuemin={SLIDER.min}
                  aria-valuemax={SLIDER.max}
                  aria-valuenow={snapValue()}
                  aria-valuetext={withUnit(snapValue())}
                  onPointerDown={dragStart}
                  onPointerMove={dragMove}
                  onPointerUp={dragEnd}
                  onPointerCancel={dragEnd}
                  onKeyDown={(e) => {
                    const keys: Record<string, number> = { ArrowRight: 1, ArrowUp: 1, ArrowLeft: -1, ArrowDown: -1, Home: -99, End: 99 };
                    if (e.key in keys) {
                      e.preventDefault();
                      takeOver();
                      nudge(keys[e.key]);
                    } else if (e.key === "Enter") {
                      e.preventDefault();
                      takeOver();
                      deliver({ label: withUnit(snapValue()), via: "keyboard" });
                    }
                  }}
                />
              </div>
              <div class="arc-item" classList={{ on: sliderOn() }} style={pos(CONFIRM_AT)}>
                <button
                  type="button"
                  class="round glass tap confirm"
                  classList={{ pressing: confirmPress() }}
                  tabIndex={sliderLive() ? 0 : -1}
                  aria-label={`Send ${withUnit(snapValue())}`}
                  onClick={() => {
                    takeOver();
                    deliver({ label: withUnit(snapValue()), via: "click" });
                  }}
                >
                  <svg viewBox="0 0 24 24" aria-hidden="true">
                    <path d="M6 12.5 10.2 16.5 18 8" />
                  </svg>
                </button>
              </div>

              {/* the curved band: live waveform while listening, "Transcribing…", or the typing field */}
              <div
                class="band-wrap"
                classList={{ on: bandOn(), tap: phase() !== "transcribing", focused: focused() && phase() === "typing" }}
                style={{ "--bx": BAND.box.x, "--by": BAND.box.y, "--bw": BAND.box.w, "--bh": BAND.box.h }}
              >
                <div class="band-glass" style={{ "-webkit-mask-image": BAND_MASK, "mask-image": BAND_MASK }} />
                <svg class="band-ink" viewBox={BAND_VIEWBOX} aria-hidden="true">
                  <path class="band-edge" d={BAND.d} />
                  <path id="peek-band-path" d={BAND_TEXT_PATH} fill="none" />
                  <g class="band-bars" classList={{ on: phase() === "listening" }}>
                    <For each={BARS}>{(b, i) => <line ref={(el) => (bars[i()] = el)} x1={b.x} y1={b.y} x2={b.x} y2={b.y} />}</For>
                  </g>
                  <Show when={phase() === "transcribing"}>
                    <text class="band-text shimmer">
                      <textPath href="#peek-band-path" startOffset="50%" text-anchor="middle">
                        Transcribing…
                      </textPath>
                    </text>
                  </Show>
                  <Show when={phase() === "typing"}>
                    <text class="band-text" classList={{ empty: !typed() }}>
                      <textPath href="#peek-band-path" startOffset="50%" text-anchor="middle">
                        {typed() ? bandText() : placeholder()}
                        <tspan class="caret" classList={{ on: focused() }}>
                          |
                        </tspan>
                      </textPath>
                    </text>
                  </Show>
                </svg>
                <Show when={phase() === "listening"}>
                  <button type="button" class="band-hit" aria-label="Stop recording" onClick={() => {
                      takeOver();
                      stopListening();
                    }} />
                </Show>
                <Show when={phase() === "typing"}>
                  <form
                    class="band-form"
                    onSubmit={(e) => {
                      e.preventDefault();
                      submitTyped();
                    }}
                  >
                    <input
                      ref={input}
                      class="band-input"
                      value={typed()}
                      aria-label={`Your answer to: ${mode() === "slider" ? SLIDER.question : QUESTION}`}
                      maxLength={60}
                      autocomplete="off"
                      spellcheck={false}
                      onInput={(e) => setTyped(e.currentTarget.value)}
                      onFocus={() => setFocused(true)}
                      onBlur={() => setFocused(false)}
                      onKeyDown={(e) => {
                        if (e.key === "Escape") {
                          e.preventDefault();
                          cancelInput();
                        }
                      }}
                    />
                  </form>
                </Show>
              </div>
              <Show when={notice()}>
                <span class="arc-item pill dark notice on" style={pos(NOTICE_AT)} role="status">
                  {notice()}
                </span>
              </Show>

              {/* the visual: the cassette from docs/drawing.md example 3 */}
              <div class="visual cassette" aria-hidden="true" ref={visualEl}>
                <div class="level-orb" />
                <div class="cassette-glass" />
                <svg class="cassette-ink" viewBox="0 0 100 100">
                  <rect x="13" y="25" width="74" height="10" rx="2" class="label" />
                  <text x="50" y="32.2" text-anchor="middle" class="label-text">
                    SIDE A
                  </text>
                  <rect x="40" y="45" width="20" height="10" rx="3" class="window" />
                  <circle cx="30" cy="50" r="9" class="hole-rim" />
                  <circle cx="70" cy="50" r="9" class="hole-rim" />
                  <g ref={reelLeft} class="reel" transform="translate(30 50)">
                    <circle r="3" />
                    <For each={[0, 1, 2, 3, 4, 5]}>{(i) => <line x1="0" y1="3" x2="0" y2="7.5" transform={`rotate(${i * 60})`} />}</For>
                  </g>
                  <g ref={reelRight} class="reel" transform="translate(70 50)">
                    <circle r="3" />
                    <For each={[0, 1, 2, 3, 4, 5]}>{(i) => <line x1="0" y1="3" x2="0" y2="7.5" transform={`rotate(${i * 60})`} />}</For>
                  </g>
                </svg>
              </div>

              {/* chrome: mic and keyboard (asks), down-arrow (always) */}
              <div class="chrome-buttons" classList={{ on: inAsk() && phase() === "asking" }}>
                <button
                  type="button"
                  class="round glass tap"
                  aria-label={`Answer by voice (in peek: ${HOTKEY.text}+5, then \\)`}
                  tabIndex={inAsk() && phase() === "asking" ? 0 : -1}
                  onClick={() => {
                    takeOver();
                    startListening();
                  }}
                >
                  <svg viewBox="0 0 24 24" aria-hidden="true">
                    <rect x="9" y="3" width="6" height="11" rx="3" />
                    <path d="M5.5 11a6.5 6.5 0 0 0 13 0M12 17.5V21" />
                  </svg>
                </button>
                <button
                  type="button"
                  class="round glass tap"
                  aria-label={`Answer by typing (in peek: ${HOTKEY.text}+5, then type)`}
                  tabIndex={inAsk() && phase() === "asking" ? 0 : -1}
                  onClick={startTyping}
                >
                  <svg viewBox="0 0 24 24" aria-hidden="true">
                    <rect x="2.5" y="6" width="19" height="12" rx="2.5" />
                    <path d="M6 10h1M9.5 10h1M13 10h1M16.5 10h1M7.5 14h9" />
                  </svg>
                </button>
              </div>
              <button type="button" class="down-arrow glass tap" aria-label="Close the bubble (double-click also stops the speech)" onClick={downArrow}>
                <svg viewBox="0 0 24 24" aria-hidden="true">
                  <path d="M6.5 9.5 12 15l5.5-5.5" />
                </svg>
              </button>
              <span class="hotkey-pill featured-key" aria-hidden="true">
                <kbd>{HOTKEY.glyphs}5</kbd> dj
              </span>
            </div>

            <Show when={answer()}>
              {(a) => (
                <div class="delivered glass" role="status">
                  <span class="tick" aria-hidden="true">✓</span>
                  <span>
                    <code>peek.ask.answered</code> → si:dj · “{a().label}” via {a().via}
                  </span>
                </div>
              )}
            </Show>
          </div>
        </div>
      </div>

      <div class="hero-copy">
        <h1 id="hero-title">
          Quick words between Carbons and <span class="serif">Silicons.</span>
        </h1>
        <p class="lede">
          A glass bubble slides in from the edge of your Mac so a Silicon can speak, show, or ask one quick question. You answer
          by voice, keyboard or click.
        </p>
        <div class="install-row" id="hero-install">
          <code class="install-code">{INSTALL_COMMAND}</code>
          <button type="button" class="copy-button" data-spacestation-event="install_command_copied" onClick={copyInstall}>
            {copied() === "copied" ? "Copied" : copied() === "manual" ? "Select & copy" : "Copy"}
          </button>
        </div>
        <p class="install-note">
          macOS 26+ · installs Honeycomb if needed, the <code>peek</code> CLI and Peek.app · no login ·{" "}
          <a href={docsHref("start")}>Start here →</a>
        </p>

        <div class="demo-controls glass">
          <div class="segmented" role="radiogroup" aria-label="Try the bubble">
            <For each={MODES}>
              {(m) => (
                <button
                  type="button"
                  role="radio"
                  aria-checked={mode() === m}
                  tabIndex={mode() === m ? 0 : -1}
                  onFocus={takeOver}
                  onKeyDown={(event) => {
                    if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
                    event.preventDefault();
                    const index = MODES.indexOf(m);
                    const next = event.key === "Home" ? 0
                      : event.key === "End" ? MODES.length - 1
                      : (index + (event.key === "ArrowRight" ? 1 : -1) + MODES.length) % MODES.length;
                    takeOver();
                    play(MODES[next]);
                    event.currentTarget.parentElement
                      ?.querySelectorAll<HTMLButtonElement>('button[role="radio"]')[next]?.focus();
                  }}
                  classList={{ active: mode() === m }}
                  onClick={() => {
                    takeOver();
                    play(m);
                  }}
                >
                  {MODE_LABEL[m]}
                </button>
              )}
            </For>
          </div>
          <code class="demo-command" tabIndex={0} aria-label={`Command for this bubble: ${COMMANDS[mode()]}`} {...reveal(() => COMMANDS[mode()], true)}>
            {COMMANDS[mode()]}
          </code>
        </div>
        <p class="sr-only" aria-live="polite">
          {announce()}
        </p>
      </div>

      {/* hover reveal and tap-to-expand live above everything, clamped to the hero */}
      <div class="reveal-layer">
        <Show when={tip()}>
          {(t) => (
            <div
              ref={tipEl}
              class="reveal-tip"
              classList={{ below: t().below, ready: t().ready, mono: t().mono }}
              style={{ left: `${t().x}px`, top: `${t().y}px` }}
              aria-hidden="true"
            >
              {t().text}
            </div>
          )}
        </Show>
        <Show when={pop()}>
          {(p) => (
            <div
              ref={popEl}
              class="reveal-pop"
              classList={{ closing: p().closing }}
              style={{
                left: `${p().left}px`,
                width: `${p().width}px`,
                top: p().fromBottom ? undefined : `${p().y}px`,
                bottom: p().fromBottom ? `${p().y}px` : undefined,
                "transform-origin": p().origin,
              }}
              aria-hidden="true"
              onClick={() => closePop()}
            >
              {p().text}
            </div>
          )}
        </Show>
      </div>
    </section>
  );
}
