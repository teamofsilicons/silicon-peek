import type { JSX } from "solid-js";

// Original artwork for the hero demo. Every drawing uses peek's own 100 × 100 unit square
// (y down), so it reads like the drawings a Silicon registers (docs/drawing.md).

/** Cover art for the fictional track "Low Tide" by The Silicons. */
export function CoverArt(): JSX.Element {
  return (
    <svg class="cover-art" viewBox="0 0 100 100" role="img" aria-label="Cover art for Low Tide by The Silicons">
      <defs>
        <linearGradient id="ca-bg" x1="0" y1="0" x2="0.3" y2="1">
          <stop offset="0" stop-color="#f8e2bf" />
          <stop offset="0.55" stop-color="#f0b48c" />
          <stop offset="1" stop-color="#d9826a" />
        </linearGradient>
        <linearGradient id="ca-sea" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stop-color="#3f6488" />
          <stop offset="1" stop-color="#22395a" />
        </linearGradient>
      </defs>
      <rect width="100" height="100" fill="url(#ca-bg)" />
      <circle cx="58" cy="44" r="21" fill="#fff3dc" />
      <circle cx="58" cy="44" r="27" fill="none" stroke="#fff3dc" stroke-opacity="0.45" stroke-width="1.2" />
      <path d="M0 63 Q12 57 25 63 T50 63 T75 63 T100 63 V100 H0 Z" fill="url(#ca-sea)" />
      <g fill="none" stroke="#a9cbe0" stroke-linecap="round">
        <path d="M6 71 Q16 67 26 71 T46 71" stroke-width="1.3" />
        <path d="M40 77 Q50 73 60 77 T80 77" stroke-width="1.1" stroke-opacity="0.8" />
        <path d="M14 83 Q22 80 30 83" stroke-width="1" stroke-opacity="0.6" />
      </g>
      <text x="8" y="15" font-family="New York, Georgia, serif" font-style="italic" font-size="9" fill="#7d3e31">
        The Silicons
      </text>
      <text x="50" y="93" text-anchor="middle" font-family="-apple-system, system-ui, sans-serif" font-weight="700" font-size="7.5" letter-spacing="2.6" fill="#f8e9d2">
        LOW TIDE
      </text>
    </svg>
  );
}

export type Ink = { ink: string };

/** Position 1: a glass orb with a voice ring (docs/drawing.md, example 2). */
export function OrbDrawing(): JSX.Element {
  return (
    <svg class="drawing drawing-orb" viewBox="0 0 100 100" aria-hidden="true">
      <circle cx="50" cy="50" r="30" class="ink-stroke" stroke-width="2.4" fill="none" />
      <path
        class="ink-stroke spin-slow"
        fill="none"
        stroke-width="1.6"
        d="M80 50 C80 58 76 66 70 71 C64 77 57 80 50 80 C42 80 35 76 29 71 C24 66 20 58 20 50 C20 42 24 34 29 29 C35 24 42 20 50 20 C58 20 65 23 71 29 C76 34 80 42 80 50 Z"
        stroke-dasharray="3 5"
      />
    </svg>
  );
}

/** Position 2: an eye that looks inward, and at the pointer (docs/drawing.md, example 6). */
export function EyeDrawing(props: { pupil?: (el: SVGCircleElement) => void }): JSX.Element {
  return (
    <svg class="drawing drawing-eye" viewBox="0 0 100 100" aria-hidden="true">
      <ellipse class="eye-white" cx="50" cy="50" rx="30" ry="22" />
      <circle ref={props.pupil} class="eye-pupil" cx="50" cy="50" r="9" />
    </svg>
  );
}

/** Position 3: a dot that pulses with any voice (docs/drawing.md, example 1). */
export function DotDrawing(): JSX.Element {
  return (
    <svg class="drawing" viewBox="0 0 100 100" aria-hidden="true">
      <circle class="ink-fill pulse" cx="50" cy="50" r="18" />
    </svg>
  );
}

/** Position 4: a vinyl record (docs/drawing.md, example 4). */
export function VinylDrawing(): JSX.Element {
  return (
    <svg class="drawing" viewBox="0 0 100 100" aria-hidden="true">
      <g class="spin-record">
        <circle cx="50" cy="50" r="44" fill="#141414" />
        <g fill="none" stroke="#ffffff" stroke-opacity="0.08" stroke-width="0.7">
          <circle cx="50" cy="50" r="38" />
          <circle cx="50" cy="50" r="33" />
          <circle cx="50" cy="50" r="28" />
          <circle cx="50" cy="50" r="23" />
        </g>
        <circle cx="50" cy="50" r="15" fill="#c8352b" />
        <path d="M50 35 A15 15 0 0 1 65 50" stroke="#f3c8a8" stroke-width="3" fill="none" />
        <circle cx="50" cy="50" r="1.8" fill="#000" />
      </g>
    </svg>
  );
}

/** Position 6: a gauge (docs/drawing.md, example 5). */
export function GaugeDrawing(): JSX.Element {
  return (
    <svg class="drawing" viewBox="0 0 100 100" aria-hidden="true">
      <path class="gauge-track" d="M26 74 A34 34 0 1 1 74 74" fill="none" stroke-width="8" stroke-linecap="round" />
      <path class="ink-stroke gauge-value" d="M26 74 A34 34 0 1 1 74 74" fill="none" stroke-width="8" stroke-linecap="round" pathLength="100" />
    </svg>
  );
}

/** Position 7: a monogram, set in SF Pro Rounded where available. */
export function MonogramDrawing(props: { letter: string }): JSX.Element {
  return (
    <svg class="drawing" viewBox="0 0 100 100" aria-hidden="true">
      <text class="ink-fill monogram" x="50" y="52" text-anchor="middle" dominant-baseline="central">
        {props.letter}
      </text>
    </svg>
  );
}

/** Position 8: a small sun whose rays turn slowly. */
export function SunDrawing(): JSX.Element {
  const rays = Array.from({ length: 10 }, (_, i) => (i * 360) / 10);
  return (
    <svg class="drawing" viewBox="0 0 100 100" aria-hidden="true">
      <circle cx="50" cy="50" r="14" fill="#ffd27a" />
      <g class="spin-slow" stroke="#ffd27a" stroke-width="3.2" stroke-linecap="round">
        {rays.map((deg) => (
          <line x1="50" y1="26" x2="50" y2="31" transform={`rotate(${deg} 50 50)`} />
        ))}
      </g>
    </svg>
  );
}
