"use client";

import { useCallback, useEffect, useLayoutEffect, useRef, useState, type CSSProperties, type FocusEvent, type MouseEvent as ReactMouseEvent, type PointerEvent as ReactPointerEvent, type ReactNode, type RefObject } from "react";
import { CopyButton } from "@/components/silicon-ui/copy-button/copy-button";
import SegmentedControl from "@/components/silicon-ui/segmented-control/segmented-control";
import { HOTKEY, INSTALL_COMMAND, docsHref } from "@/lib/site";
import { CoverArt, DotDrawing, EyeDrawing, GaugeDrawing, MonogramDrawing, OrbDrawing, SunDrawing, VinylDrawing } from "./Art";
import { SLOTS, arcFraction, arcPath, arcPlacement, arcPoint, arcSpanPath, curvedBand, snapToStep, type SlotGeometry } from "./geometry";
import Wallpaper from "./Wallpaper";

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
const cx = (...names: (string | false | null | undefined)[]) => names.filter(Boolean).join(" ");

/** State that timers and the animation loop read synchronously, without waiting for a render. */
function useLive<T>(initial: T) {
  const [value, setValue] = useState(initial);
  const ref = useRef(value);
  const set = useCallback((next: T) => {
    ref.current = next;
    setValue(next);
  }, []);
  return [value, set, ref] as const;
}

// ---------------------------------------------------------------------------------------------
// Idle positions
// ---------------------------------------------------------------------------------------------

type Resident = { name: string; drawing: (pupil: RefObject<SVGCircleElement | null>) => ReactNode };

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

function IdleSlot({ slot, resident, pupil }: { slot: SlotGeometry; resident: Resident; pupil: RefObject<SVGCircleElement | null> }) {
  const [bump, setBump] = useState(false);
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);
  const d = arcPath(slot.facing, IDLE_ARC.radius, IDLE_ARC.spread, IDLE_ARC.pivot);
  // Push the hotkey pill past the arc, further on the sides where the pill is wide along the facing.
  const extra = 0.2 + 0.42 * Math.abs(Math.cos(slot.facing));
  const pill = arcPlacement(slot.facing, IDLE_ARC.radius + extra, 0, IDLE_ARC.pivot);
  return (
    <div className="slot idle" data-index={slot.index} data-side={slot.side}>
      <svg className="arc-guide" viewBox="-3.2 -3.2 6.4 6.4" aria-hidden="true">
        <path d={d} />
      </svg>
      <button
        type="button"
        className={cx("visual glass circle", bump && "bump")}
        aria-label={`Position ${slot.index}, ${slot.side}: ${resident.name}, shortcut ${HOTKEY.text}+${slot.index}. Clicking a visual only animates it.`}
        onClick={() => {
          setBump(false);
          requestAnimationFrame(() => setBump(true));
          clearTimeout(timer.current);
          timer.current = window.setTimeout(() => setBump(false), 420);
        }}
      >
        {resident.drawing(pupil)}
      </button>
      <span className="hotkey-pill" style={{ "--x": pill.x, "--y": pill.y } as CSSProperties} aria-hidden="true">
        <kbd>
          {HOTKEY.glyphs}
          {slot.index}
        </kbd>{" "}
        {resident.name.slice(3)}
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
const MIN_BOUND_AT = arcPlacement(UP, ASK_ARC.radius + SCALE_LIFT, TRACK.from, ASK_ARC.pivot);
const MAX_BOUND_AT = arcPlacement(UP, ASK_ARC.radius + SCALE_LIFT, TRACK.to, ASK_ARC.pivot);

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

const pos = (p: { x: number; y: number; rotate?: number }) => ({ "--x": p.x, "--y": p.y, "--r": `${p.rotate ?? 0}deg` }) as CSSProperties;

// ---------------------------------------------------------------------------------------------
// Reveal: cut-off text shows in full on hover; long text opens in place when clicked
// ---------------------------------------------------------------------------------------------

type Tip = { text: string; x: number; y: number; below: boolean; mono: boolean; ready: boolean; anchor: { top: number; bottom: number } };
type Pop = { text: string; left: number; y: number; fromBottom: boolean; width: number; origin: string; closing: boolean; source: HTMLElement };

const isCut = (el: HTMLElement) => el.scrollWidth > el.clientWidth + 1 || el.scrollHeight > el.clientHeight + 1;

export default function Hero() {
  const [mode, setMode, modeRef] = useLive<Mode>("show");
  const [phase, setPhase, phaseRef] = useLive<Phase>("showing");
  const [out, setOut] = useState(false);
  const [answer, setAnswer] = useState<Answer | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [typed, setTyped] = useState("");
  const [focused, setFocused] = useState(false);
  const [highlight, setHighlight] = useState<string | null>(null);
  const [, setAutoplay, autoplayRef] = useLive(true);
  const [captionCut, setCaptionCut] = useState(false);
  const [noteCut, setNoteCut] = useState(false);
  const [frac, setFrac, fracRef] = useLive(fracOf(SLIDER.initial));
  const [dragging, setDragging, draggingRef] = useLive(false);
  const [confirmPress, setConfirmPress] = useState(false);
  const [tip, setTip] = useState<Tip | null>(null);
  const [pop, setPop, popRef] = useLive<Pop | null>(null);

  const reduceMotion = useRef(false);
  const timers = useRef<number[]>([]);
  const later = (ms: number, fn: () => void) => void timers.current.push(window.setTimeout(fn, ms));
  /** A step of the automatic tour: skipped once the visitor has taken over. */
  const auto = (ms: number, fn: () => void) => later(ms, () => autoplayRef.current && fn());
  const clearTimers = () => {
    timers.current.forEach(clearTimeout);
    timers.current = [];
  };

  const heroRef = useRef<HTMLElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const reelLeft = useRef<SVGGElement>(null);
  const reelRight = useRef<SVGGElement>(null);
  const stageRef = useRef<HTMLDivElement>(null);
  const bubbleRef = useRef<HTMLDivElement>(null);
  const visualRef = useRef<HTMLDivElement>(null);
  const pupilRef = useRef<SVGCircleElement>(null);
  const tipRef = useRef<HTMLDivElement>(null);
  const popElRef = useRef<HTMLDivElement>(null);
  const captionText = useRef<HTMLSpanElement>(null);
  const noteText = useRef<HTMLSpanElement>(null);
  const bars = useRef<(SVGLineElement | null)[]>([]);
  const history = useRef<number[]>(Array(BAR_COUNT).fill(0));

  // ------------------------------------------------------------------ the slider's motion
  const springRaf = useRef(0);
  const simRaf = useRef(0);
  const velocity = useRef(0);
  const stopMotion = () => {
    cancelAnimationFrame(springRaf.current);
    cancelAnimationFrame(simRaf.current);
    springRaf.current = simRaf.current = 0;
  };

  /** Springs the thumb to `target` (a fraction), carrying the drag's velocity, with a little bounce. */
  function springTo(target: number) {
    cancelAnimationFrame(springRaf.current);
    if (reduceMotion.current) {
      setFrac(target);
      return;
    }
    let x = fracRef.current;
    let v = clamp(velocity.current, -6, 6);
    let last = performance.now();
    const step = (now: number) => {
      const dt = Math.min(0.032, (now - last) / 1000);
      last = now;
      v += (-420 * (x - target) - 21 * v) * dt;
      x += v * dt;
      if (Math.abs(x - target) < 0.0006 && Math.abs(v) < 0.01) {
        setFrac(target);
        springRaf.current = 0;
        return;
      }
      setFrac(clamp(x, -0.03, 1.03));
      springRaf.current = requestAnimationFrame(step);
    };
    springRaf.current = requestAnimationFrame(step);
  }

  const snapValue = () => valueOf(fracRef.current);

  /** The fraction along the track under a pointer, following it continuously (no per-step jumps). */
  function pointerFraction(e: ReactPointerEvent): number {
    const o = bubbleRef.current!.getBoundingClientRect();
    const v = visualRef.current!.getBoundingClientRect().width || 1;
    return arcFraction(UP, ASK_ARC.pivot, TRACK.from, TRACK.to, { x: (e.clientX - o.left) / v, y: (e.clientY - o.top) / v });
  }

  const lastMove = useRef({ f: 0, t: 0 });
  function dragStart(e: ReactPointerEvent) {
    if (!(modeRef.current === "slider" && phaseRef.current === "asking")) return;
    e.preventDefault();
    takeOver();
    stopMotion();
    (e.currentTarget as Element).setPointerCapture(e.pointerId);
    setDragging(true);
    velocity.current = 0;
    const f = pointerFraction(e);
    lastMove.current = { f, t: performance.now() };
    setFrac(f);
  }
  function dragMove(e: ReactPointerEvent) {
    if (!draggingRef.current) return;
    const f = pointerFraction(e);
    const now = performance.now();
    const dt = Math.max(0.008, (now - lastMove.current.t) / 1000);
    velocity.current = velocity.current * 0.6 + ((f - lastMove.current.f) / dt) * 0.4;
    lastMove.current = { f, t: now };
    setFrac(f);
  }
  function dragEnd() {
    if (!draggingRef.current) return;
    setDragging(false);
    if (performance.now() - lastMove.current.t > 90) velocity.current = 0;
    springTo(fracOf(snapValue())); // snap to the nearest step, with a spring
  }

  function nudge(steps: number) {
    velocity.current = 0;
    springTo(fracOf(snapToStep(snapValue() + steps * SLIDER.step, SLIDER.min, SLIDER.max, SLIDER.step)));
  }

  /** The tour's drag: the thumb glides with the "pointer", then springs to the nearest step. */
  function simulateDrag() {
    const start = fracRef.current;
    const end = 0.73;
    const began = performance.now();
    setDragging(true);
    const step = (now: number) => {
      const t = Math.min(1, (now - began) / 1150);
      const ease = t < 0.5 ? 4 * t * t * t : 1 - (-2 * t + 2) ** 3 / 2;
      setFrac(start + (end - start) * ease + Math.sin(t * Math.PI) * 0.015);
      if (t < 1) simRaf.current = requestAnimationFrame(step);
      else {
        simRaf.current = 0;
        setDragging(false);
        velocity.current = 0.5;
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
    simRaf.current = requestAnimationFrame(step);
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
    if (autoplayRef.current) setAutoplay(false);
    cancelAnimationFrame(simRaf.current);
    simRaf.current = 0;
  }

  function deliver(a: Answer) {
    clearTimers();
    stopMotion();
    setAnswer(a);
    setPhase("leaving");
    later(450, () => setOut(true));
    later(3600, () => play(autoplayRef.current ? NEXT[modeRef.current] : modeRef.current));
  }

  function startListening() {
    setNotice(null);
    setPhase("listening");
    auto(2300, () => stopListening());
  }

  function stopListening() {
    setPhase("transcribing");
    // The real app uploads the finished WAV once; here the "transcript" is simulated.
    later(1100, () => deliver(modeRef.current === "slider" ? { label: withUnit(8), via: "voice" } : { label: OPTIONS[0], via: "voice" }));
  }

  function startTyping() {
    takeOver();
    setNotice(null);
    setPhase("typing");
  }

  function submitTyped() {
    if (modeRef.current === "slider") {
      const n = matchNumber(typed);
      if (n !== null) deliver({ label: withUnit(n), via: "keyboard" });
      else setNotice(NO_MATCH);
      return;
    }
    const match = matchOption(typed);
    if (match) deliver({ label: match, via: "keyboard" });
    else setNotice(NO_MATCH);
  }

  function cancelInput() {
    setNotice(null);
    setTyped("");
    setPhase("asking");
  }

  const arrowTimer = useRef<number | undefined>(undefined);
  function downArrow(e: ReactMouseEvent) {
    takeOver();
    clearTimeout(arrowTimer.current);
    const stopAudio = e.detail >= 2;
    const slide = () => {
      clearTimers();
      stopMotion();
      closePop(true);
      if (stopAudio && phaseRef.current === "speaking") setPhase("showing");
      setOut(true);
      later(2600, () => play(modeRef.current));
    };
    if (stopAudio) slide();
    else arrowTimer.current = window.setTimeout(slide, 260);
  }

  // ------------------------------------------------------------------ reveal: tooltip and popup
  const tipTimer = useRef<number | undefined>(undefined);
  function hideTip() {
    clearTimeout(tipTimer.current);
    setTip(null);
  }
  /** Shows the complete text of a cut-off element in a glass tooltip, placed toward the screen centre. */
  function showTip(el: HTMLElement, text: string, mono = false, cutEl: HTMLElement = el) {
    clearTimeout(tipTimer.current);
    tipTimer.current = window.setTimeout(() => {
      if (popRef.current || !isCut(cutEl) || !heroRef.current) return;
      const h = heroRef.current.getBoundingClientRect();
      const r = el.getBoundingClientRect();
      const below = (r.top + r.bottom) / 2 < h.top + h.height / 2;
      const anchor = { top: r.top - h.top, bottom: r.bottom - h.top };
      setTip({ text, x: r.left + r.width / 2 - h.left, y: below ? anchor.bottom + 8 : anchor.top - 8, below, mono, ready: false, anchor });
    }, 70);
  }
  // Once the tooltip has a size, keep it inside the hero: flip it to the other side or slide it along.
  useLayoutEffect(() => {
    if (!tip || tip.ready || !tipRef.current || !heroRef.current) return;
    const h = heroRef.current.getBoundingClientRect();
    const w = tipRef.current.offsetWidth;
    const th = tipRef.current.offsetHeight;
    let { below, y } = tip;
    if (!below && y - th < 6) [below, y] = [true, tip.anchor.bottom + 8];
    else if (below && y + th > h.height - 6) [below, y] = [false, tip.anchor.top - 8];
    setTip({ ...tip, x: clamp(tip.x, w / 2 + 10, h.width - w / 2 - 10), y, below, ready: true });
  }, [tip]);
  /** Hover and focus handlers for an element whose text may be cut off (in `cutEl`, or itself). */
  const reveal = (text: string, mono = false, cutEl?: RefObject<HTMLElement | null>) => ({
    onPointerEnter: (e: ReactPointerEvent<HTMLElement>) => showTip(e.currentTarget, text, mono, cutEl?.current ?? undefined),
    onPointerLeave: hideTip,
    onFocus: (e: FocusEvent<HTMLElement>) => showTip(e.currentTarget, text, mono, cutEl?.current ?? undefined),
    onBlur: hideTip,
  });

  /** Opens long text in a small glass popup right where it is, growing away from the nearer edge. */
  function togglePop(el: HTMLElement, text: string, cutEl: HTMLElement | null) {
    const open = popRef.current;
    if (open && !open.closing && open.source === el) {
      closePop();
      return;
    }
    if (!cutEl || !isCut(cutEl) || !heroRef.current) return;
    clearTimeout(popTimer.current);
    hideTip();
    const h = heroRef.current.getBoundingClientRect();
    const r = el.getBoundingClientRect();
    const width = Math.min(260, h.width - 24);
    const centre = r.left + r.width / 2 - h.left;
    const left = clamp(centre - width / 2, 12, h.width - width - 12);
    const fromBottom = (r.top + r.bottom) / 2 > h.top + h.height / 2;
    setPop({
      text,
      left,
      width,
      fromBottom,
      y: Math.max(8, fromBottom ? h.bottom - r.bottom - 4 : r.top - h.top - 4),
      origin: `${Math.round(centre - left)}px ${fromBottom ? `calc(100% - ${Math.round(r.height / 2 + 4)}px)` : `${Math.round(r.height / 2 + 4)}px`}`,
      closing: false,
      source: el,
    });
  }
  const popTimer = useRef<number | undefined>(undefined);
  function closePop(now = false) {
    const p = popRef.current;
    if (!p) return;
    clearTimeout(popTimer.current);
    if (now || reduceMotion.current) {
      setPop(null);
      return;
    }
    setPop({ ...p, closing: true });
    popTimer.current = window.setTimeout(() => setPop(null), 220);
  }

  const announce = (() => {
    if (answer) return `Answer delivered to si:dj as peek.ask.answered: ${answer.label}, via ${answer.via}.`;
    if (out) return "The bubble slid back.";
    switch (phase) {
      case "showing":
        return mode === "show" ? `Showing: ${CAPTION_TEXT}, 2022. ${NOTE}` : "Speech finished.";
      case "speaking":
        return "Speaking.";
      case "asking":
        return mode === "slider"
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
  })();

  // The latest handlers, for the listeners registered once on mount.
  const live = useRef({ play, closePop, hideTip });
  live.current = { play, closePop, hideTip };

  // One animation loop for the cassette reels, the level orb, the mic waveform and the eye.
  useEffect(() => {
    reduceMotion.current = matchMedia("(prefers-reduced-motion: reduce)").matches;
    if (reduceMotion.current) setAutoplay(false);
    let raf = 0;
    let visible = true;
    let pointer: { x: number; y: number } | null = null;
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
      const p = phaseRef.current;
      const voiced = p === "speaking" || p === "listening";
      const target = voiced && !reduceMotion.current ? syntheticLevel(t) : 0;
      level += (target - level) * Math.min(1, dt * 10);
      spin += (voiced && !reduceMotion.current ? 2 + level * 8 : 0) * dt;
      const deg = (spin * 180) / Math.PI;
      reelLeft.current?.setAttribute("transform", `translate(30 50) rotate(${deg.toFixed(2)})`);
      reelRight.current?.setAttribute("transform", `translate(70 50) rotate(${deg.toFixed(2)})`);
      bubbleRef.current?.style.setProperty("--level", level.toFixed(3));
      if (p === "listening") {
        history.current.push(level);
        history.current.shift();
        bars.current.forEach((bar, i) => {
          if (!bar) return;
          const b = BARS[i];
          const half = 0.012 + Math.max(0.05, history.current[i]) * 0.1;
          bar.setAttribute("x1", (b.x - b.ux * half).toFixed(4));
          bar.setAttribute("y1", (b.y - b.uy * half).toFixed(4));
          bar.setAttribute("x2", (b.x + b.ux * half).toFixed(4));
          bar.setAttribute("y2", (b.y + b.uy * half).toFixed(4));
        });
      }
      const pupil = pupilRef.current;
      if (pupil) {
        let ax = Math.cos((3 * Math.PI) / 4) * 6;
        let ay = Math.sin((3 * Math.PI) / 4) * 6;
        if (pointer) {
          const box = (pupil.ownerSVGElement as SVGSVGElement).getBoundingClientRect();
          const dx = pointer.x - (box.left + box.width / 2);
          const dy = pointer.y - (box.top + box.height / 2);
          const dist = Math.hypot(dx, dy) / (box.width / 100);
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
    const observer = new IntersectionObserver((entries) => {
      visible = entries.some((entry) => entry.isIntersecting);
      resume();
    });
    if (stageRef.current) observer.observe(stageRef.current);
    // The popup closes on a click outside it (a click on its own text is handled by togglePop) or Esc.
    const onDown = (e: PointerEvent) => {
      const p = popRef.current;
      if (!p || p.closing) return;
      const target = e.target as Node;
      if (popElRef.current?.contains(target) || p.source.contains(target)) return;
      live.current.closePop();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && popRef.current) {
        live.current.closePop();
        live.current.hideTip();
      }
    };
    const onResize = () => {
      live.current.closePop(true);
      live.current.hideTip();
    };
    window.addEventListener("pointermove", onPointer, { passive: true });
    document.addEventListener("pointerdown", onDown, true);
    document.addEventListener("keydown", onKey);
    window.addEventListener("resize", onResize);
    document.addEventListener("visibilitychange", resume);
    resume();
    // ?demo=show|speak|ask|slider opens the demo in that state (and stops the automatic cycle).
    const requested = new URLSearchParams(location.search).get("demo");
    if (requested && (MODES as readonly string[]).includes(requested)) {
      setAutoplay(false);
      live.current.play(requested as Mode);
    } else live.current.play("show");
    const pending = timers.current;
    return () => {
      cancelAnimationFrame(raf);
      cancelAnimationFrame(springRaf.current);
      cancelAnimationFrame(simRaf.current);
      observer.disconnect();
      window.removeEventListener("pointermove", onPointer);
      document.removeEventListener("pointerdown", onDown, true);
      document.removeEventListener("keydown", onKey);
      window.removeEventListener("resize", onResize);
      document.removeEventListener("visibilitychange", resume);
      pending.forEach(clearTimeout);
      timers.current.forEach(clearTimeout);
      clearTimeout(arrowTimer.current);
      clearTimeout(tipTimer.current);
      clearTimeout(popTimer.current);
    };
  }, [setAutoplay, phaseRef, popRef]);

  // Keeps captionCut and noteCut in sync with whether the text is cut off (its size follows the screen).
  useEffect(() => {
    const watch = (el: HTMLElement | null, set: (cut: boolean) => void) => {
      if (!el) return () => {};
      const check = () => set(isCut(el));
      const ro = new ResizeObserver(check);
      ro.observe(el);
      const frame = requestAnimationFrame(check);
      return () => {
        ro.disconnect();
        cancelAnimationFrame(frame);
      };
    };
    const stopCaption = watch(captionText.current, setCaptionCut);
    const stopNote = watch(noteText.current, setNoteCut);
    return () => {
      stopCaption();
      stopNote();
    };
  }, []);

  // The typing field appears with the "typing" phase; focus it once it is there.
  useEffect(() => {
    if (phase === "typing") inputRef.current?.focus({ preventScroll: true });
  }, [phase]);

  const inAsk = mode === "ask" || mode === "slider";
  const asking = ["asking", "listening", "typing", "transcribing"].includes(phase);
  const showOn = mode === "show";
  const showLive = showOn && !out && phase !== "leaving";
  const choiceOn = mode === "ask" && (phase === "asking" || phase === "speaking");
  const sliderOn = mode === "slider" && (phase === "asking" || phase === "speaking");
  const sliderLive = mode === "slider" && phase === "asking";
  const bandOn = ["listening", "typing", "transcribing"].includes(phase);
  const value = valueOf(frac);
  const thumbAt = arcPlacement(UP, ASK_ARC.radius, trackOffset(frac), ASK_ARC.pivot);
  const valueAt = arcPlacement(UP, ASK_ARC.radius + SCALE_LIFT, trackOffset(clamp(frac, 0, 1)), ASK_ARC.pivot);
  const fillPath = arcSpanPath(UP, ASK_ARC.radius, TRACK.from, trackOffset(clamp(frac, 0, 1)), ASK_ARC.pivot);
  const bandText = typed.length > BAND_CHARS ? `…${typed.slice(-(BAND_CHARS - 1))}` : typed;
  const placeholder = mode === "slider" ? "Type a number, like “6”…" : "Type “play it” or “skip”…";
  const popOpenFor = (text: string) => pop?.text === text && !pop.closing;

  return (
    <section className="hero" id="top" aria-labelledby="hero-title" ref={heroRef}>
      <div className="desktop" ref={stageRef}>
        <Wallpaper />
        <div className="slots" role="group" aria-label="The eight peek positions around a Mac screen">
          {SLOTS.filter((s) => s.index !== 5).map((slot) => (
            <IdleSlot key={slot.index} slot={slot} resident={RESIDENTS[slot.index]} pupil={pupilRef} />
          ))}

          <div className="slot featured" data-index="5" data-side="bottom">
            {/* No panel and no background: only the visual, the arc and its elements, over the desktop. */}
            <div
              ref={bubbleRef}
              className={cx("bubble", out && "out", `mode-${mode}`, `phase-${phase}`)}
              onPointerDown={takeOver}
              onKeyDown={(e) => {
                if (e.key === "Escape" && ["listening", "typing"].includes(phaseRef.current)) {
                  e.preventDefault();
                  takeOver();
                  cancelInput();
                }
              }}
            >
              {/* information arc: show. The year is static; cut-off text is interactive. */}
              <span className={cx("arc-item pill dark year", showOn && "on")} style={pos(YEAR)}>
                2022
              </span>
              <div className={cx("arc-item cover-group", showOn && "on")} style={pos(CAPTION)}>
                <div className="cover-frame">
                  <CoverArt />
                </div>
                <button
                  type="button"
                  className={cx("pill dark caption", captionCut && "tap", popOpenFor(CAPTION_TEXT) && "open")}
                  tabIndex={showLive && captionCut ? 0 : -1}
                  aria-expanded={captionCut ? pop?.text === CAPTION_TEXT : undefined}
                  {...reveal(CAPTION_TEXT, false, captionText)}
                  onClick={(e) => {
                    takeOver();
                    togglePop(e.currentTarget, CAPTION_TEXT, captionText.current);
                  }}
                >
                  <span className="clip-line" ref={captionText}>
                    {CAPTION_TEXT}
                  </span>
                </button>
              </div>
              <div className={cx("arc-item note-group", showOn && "on")} style={pos(NOTE_AT)}>
                <button
                  type="button"
                  className={cx("pill dark note", noteCut && "tap", popOpenFor(NOTE) && "open")}
                  tabIndex={showLive && noteCut ? 0 : -1}
                  aria-expanded={noteCut ? pop?.text === NOTE : undefined}
                  {...reveal(NOTE, false, noteText)}
                  onClick={(e) => {
                    takeOver();
                    togglePop(e.currentTarget, NOTE, noteText.current);
                  }}
                >
                  <span className="clip-lines" ref={noteText}>
                    {NOTE}
                  </span>
                </button>
              </div>

              {/* question arc, and the curved answer band right under it */}
              <svg className="ask-arcs" viewBox="-2.2 -2.2 4.4 2.4" aria-hidden="true">
                <path id="peek-question-path" d={QUESTION_PATH} fill="none" />
                <text className={cx("question-text", inAsk && asking && "on")}>
                  <textPath href="#peek-question-path" startOffset="50%" textAnchor="middle">
                    {mode === "slider" ? SLIDER.question : QUESTION}
                  </textPath>
                </text>
                {/* slider: the track follows the arc; dots mark the steps */}
                <g className={cx("scale", sliderOn && "on")}>
                  <path className="track-edge" d={TRACK_PATH} />
                  <path className="track" d={TRACK_PATH} />
                  <path className="track-fill" d={fillPath} />
                  {STEP_DOTS.map((v) => {
                    const p = arcPoint(UP, ASK_ARC.radius, trackOffset(fracOf(v)), ASK_ARC.pivot);
                    return <circle key={v} className={cx("step-dot", v <= value && "lit", dragging && v === value && "next")} cx={p.x} cy={p.y} r={0.024} />;
                  })}
                  <path className="track-hit" d={TRACK_PATH} onPointerDown={dragStart} onPointerMove={dragMove} onPointerUp={dragEnd} onPointerCancel={dragEnd} />
                </g>
              </svg>

              {/* single choice */}
              {OPTIONS.map((label, i) => (
                <div key={label} className={cx("arc-item", choiceOn && "on")} style={pos(OPTION_AT[i])}>
                  <button
                    type="button"
                    className={cx("pill glass option tap", highlight === label && "hot")}
                    tabIndex={mode === "ask" && phase === "asking" ? 0 : -1}
                    aria-hidden={!(mode === "ask" && phase === "asking")}
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
              ))}

              {/* slider: bounds, the live value, the thumb and the ✓ */}
              <span className={cx("arc-item bound", sliderOn && frac > 0.2 && "on")} style={pos(MIN_BOUND_AT)}>
                {withUnit(SLIDER.min)}
              </span>
              <span className={cx("arc-item bound", sliderOn && frac < 0.8 && "on")} style={pos(MAX_BOUND_AT)}>
                {withUnit(SLIDER.max)}
              </span>
              <span className={cx("arc-item pill glass value-pill", sliderOn && "on", dragging && "live")} style={pos(valueAt)} aria-hidden="true">
                {withUnit(value)}
              </span>
              <div className={cx("arc-item thumb-anchor", sliderOn && "on")} style={pos(thumbAt)}>
                <span
                  className={cx("thumb glass tap", dragging && "dragging")}
                  role="slider"
                  tabIndex={sliderLive ? 0 : -1}
                  aria-label={SLIDER.question}
                  aria-valuemin={SLIDER.min}
                  aria-valuemax={SLIDER.max}
                  aria-valuenow={value}
                  aria-valuetext={withUnit(value)}
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
              <div className={cx("arc-item", sliderOn && "on")} style={pos(CONFIRM_AT)}>
                <button
                  type="button"
                  className={cx("round glass tap confirm", confirmPress && "pressing")}
                  tabIndex={sliderLive ? 0 : -1}
                  aria-label={`Send ${withUnit(value)}`}
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
                className={cx("band-wrap", bandOn && "on", phase !== "transcribing" && "tap", focused && phase === "typing" && "focused")}
                style={{ "--bx": BAND.box.x, "--by": BAND.box.y, "--bw": BAND.box.w, "--bh": BAND.box.h } as CSSProperties}
              >
                <div className="band-glass" style={{ WebkitMaskImage: BAND_MASK, maskImage: BAND_MASK }} />
                <svg className="band-ink" viewBox={BAND_VIEWBOX} aria-hidden="true">
                  <path className="band-edge" d={BAND.d} />
                  <path id="peek-band-path" d={BAND_TEXT_PATH} fill="none" />
                  <g className={cx("band-bars", phase === "listening" && "on")}>
                    {BARS.map((b, i) => (
                      <line
                        key={i}
                        ref={(el) => {
                          bars.current[i] = el;
                        }}
                        x1={b.x}
                        y1={b.y}
                        x2={b.x}
                        y2={b.y}
                      />
                    ))}
                  </g>
                  {phase === "transcribing" && (
                    <text className="band-text shimmer">
                      <textPath href="#peek-band-path" startOffset="50%" textAnchor="middle">
                        Transcribing…
                      </textPath>
                    </text>
                  )}
                  {phase === "typing" && (
                    <text className={cx("band-text", !typed && "empty")}>
                      <textPath href="#peek-band-path" startOffset="50%" textAnchor="middle">
                        {typed ? bandText : placeholder}
                        <tspan className={cx("caret", focused && "on")}>|</tspan>
                      </textPath>
                    </text>
                  )}
                </svg>
                {phase === "listening" && (
                  <button
                    type="button"
                    className="band-hit"
                    aria-label="Stop recording"
                    onClick={() => {
                      takeOver();
                      stopListening();
                    }}
                  />
                )}
                {phase === "typing" && (
                  <form
                    className="band-form"
                    onSubmit={(e) => {
                      e.preventDefault();
                      submitTyped();
                    }}
                  >
                    <input
                      ref={inputRef}
                      className="band-input"
                      value={typed}
                      aria-label={`Your answer to: ${mode === "slider" ? SLIDER.question : QUESTION}`}
                      maxLength={60}
                      autoComplete="off"
                      spellCheck={false}
                      onChange={(e) => setTyped(e.currentTarget.value)}
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
                )}
              </div>
              {notice && (
                <span className="arc-item pill dark notice on" style={pos(NOTICE_AT)} role="status">
                  {notice}
                </span>
              )}

              {/* the visual: the cassette from docs/drawing.md example 3 */}
              <div className="visual cassette" aria-hidden="true" ref={visualRef}>
                <div className="level-orb" />
                <div className="cassette-glass" />
                <svg className="cassette-ink" viewBox="0 0 100 100">
                  <rect x="13" y="25" width="74" height="10" rx="2" className="label" />
                  <text x="50" y="32.2" textAnchor="middle" className="label-text">
                    SIDE A
                  </text>
                  <rect x="40" y="45" width="20" height="10" rx="3" className="window" />
                  <circle cx="30" cy="50" r="9" className="hole-rim" />
                  <circle cx="70" cy="50" r="9" className="hole-rim" />
                  <g ref={reelLeft} className="reel" transform="translate(30 50)">
                    <circle r="3" />
                    {[0, 1, 2, 3, 4, 5].map((i) => (
                      <line key={i} x1="0" y1="3" x2="0" y2="7.5" transform={`rotate(${i * 60})`} />
                    ))}
                  </g>
                  <g ref={reelRight} className="reel" transform="translate(70 50)">
                    <circle r="3" />
                    {[0, 1, 2, 3, 4, 5].map((i) => (
                      <line key={i} x1="0" y1="3" x2="0" y2="7.5" transform={`rotate(${i * 60})`} />
                    ))}
                  </g>
                </svg>
              </div>

              {/* chrome: mic and keyboard (asks), down-arrow (always) */}
              <div className={cx("chrome-buttons", inAsk && phase === "asking" && "on")}>
                <button
                  type="button"
                  className="round glass tap"
                  aria-label={`Answer by voice (in peek: ${HOTKEY.text}+5, then \\)`}
                  tabIndex={inAsk && phase === "asking" ? 0 : -1}
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
                  className="round glass tap"
                  aria-label={`Answer by typing (in peek: ${HOTKEY.text}+5, then type)`}
                  tabIndex={inAsk && phase === "asking" ? 0 : -1}
                  onClick={startTyping}
                >
                  <svg viewBox="0 0 24 24" aria-hidden="true">
                    <rect x="2.5" y="6" width="19" height="12" rx="2.5" />
                    <path d="M6 10h1M9.5 10h1M13 10h1M16.5 10h1M7.5 14h9" />
                  </svg>
                </button>
              </div>
              <button type="button" className="down-arrow glass tap" aria-label="Close the bubble (double-click also stops the speech)" onClick={downArrow}>
                <svg viewBox="0 0 24 24" aria-hidden="true">
                  <path d="M6.5 9.5 12 15l5.5-5.5" />
                </svg>
              </button>
              <span className="hotkey-pill featured-key" aria-hidden="true">
                <kbd>{HOTKEY.glyphs}5</kbd> dj
              </span>
            </div>

            {answer && (
              <div className="delivered glass" role="status">
                <span className="tick" aria-hidden="true">
                  ✓
                </span>
                <span>
                  <code>peek.ask.answered</code> → si:dj · “{answer.label}” via {answer.via}
                </span>
              </div>
            )}
          </div>
        </div>
      </div>

      <div className="hero-copy">
        <h1 id="hero-title">
          Quick words between Carbons and <span className="serif">Silicons.</span>
        </h1>
        <p className="lede">
          A glass bubble slides in from the edge of your Mac so a Silicon can speak, show, or ask one quick question. You answer by voice,
          keyboard or click.
        </p>
        <div className="install-row" id="hero-install">
          <code className="install-code">{INSTALL_COMMAND}</code>
          <CopyButton value={INSTALL_COMMAND} label="Copy" />
        </div>
        <p className="install-note">
          macOS 26+ · signed native <code>peek</code> CLI and Peek.app · Silicon Accounts · <a href={docsHref("start")}>Start here →</a>
        </p>

        <div className="demo-controls glass" onFocus={takeOver}>
          <SegmentedControl
            label="Try the bubble"
            options={MODES.map((m) => ({ value: m, label: MODE_LABEL[m] }))}
            value={mode}
            onValueChange={(next) => {
              takeOver();
              play(next as Mode);
            }}
          />
          <code className="demo-command" tabIndex={0} aria-label={`Command for this bubble: ${COMMANDS[mode]}`} {...reveal(COMMANDS[mode], true)}>
            {COMMANDS[mode]}
          </code>
        </div>
        <p className="sr-only" aria-live="polite">
          {announce}
        </p>
      </div>

      {/* hover reveal and tap-to-expand live above everything, clamped to the hero */}
      <div className="reveal-layer">
        {tip && (
          <div ref={tipRef} className={cx("reveal-tip", tip.below && "below", tip.ready && "ready", tip.mono && "mono")} style={{ left: `${tip.x}px`, top: `${tip.y}px` }} aria-hidden="true">
            {tip.text}
          </div>
        )}
        {pop && (
          <div
            ref={popElRef}
            className={cx("reveal-pop", pop.closing && "closing")}
            style={{
              left: `${pop.left}px`,
              width: `${pop.width}px`,
              top: pop.fromBottom ? undefined : `${pop.y}px`,
              bottom: pop.fromBottom ? `${pop.y}px` : undefined,
              transformOrigin: pop.origin,
            }}
            aria-hidden="true"
            onClick={() => closePop()}
          >
            {pop.text}
          </div>
        )}
      </div>
    </section>
  );
}
