import type { Ref } from "react";

// Original artwork for the hero demo. Every drawing uses peek's own 100 × 100 unit square
// (y down), so it reads like the drawings a Silicon registers (docs/drawing.md).

/** Cover art for the fictional track "Low Tide" by The Silicons. */
export function CoverArt() {
  return (
    <svg className="cover-art" viewBox="0 0 100 100" role="img" aria-label="Cover art for Low Tide by The Silicons">
      <defs>
        <linearGradient id="ca-bg" x1="0" y1="0" x2="0.3" y2="1">
          <stop offset="0" stopColor="#f8e2bf" />
          <stop offset="0.55" stopColor="#f0b48c" />
          <stop offset="1" stopColor="#d9826a" />
        </linearGradient>
        <linearGradient id="ca-sea" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="#3f6488" />
          <stop offset="1" stopColor="#22395a" />
        </linearGradient>
      </defs>
      <rect width="100" height="100" fill="url(#ca-bg)" />
      <circle cx="58" cy="44" r="21" fill="#fff3dc" />
      <circle cx="58" cy="44" r="27" fill="none" stroke="#fff3dc" strokeOpacity="0.45" strokeWidth="1.2" />
      <path d="M0 63 Q12 57 25 63 T50 63 T75 63 T100 63 V100 H0 Z" fill="url(#ca-sea)" />
      <g fill="none" stroke="#a9cbe0" strokeLinecap="round">
        <path d="M6 71 Q16 67 26 71 T46 71" strokeWidth="1.3" />
        <path d="M40 77 Q50 73 60 77 T80 77" strokeWidth="1.1" strokeOpacity="0.8" />
        <path d="M14 83 Q22 80 30 83" strokeWidth="1" strokeOpacity="0.6" />
      </g>
      <text x="8" y="15" fontFamily="New York, Georgia, serif" fontStyle="italic" fontSize="9" fill="#7d3e31">
        The Silicons
      </text>
      <text x="50" y="93" textAnchor="middle" fontFamily="-apple-system, system-ui, sans-serif" fontWeight="700" fontSize="7.5" letterSpacing="2.6" fill="#f8e9d2">
        LOW TIDE
      </text>
    </svg>
  );
}

/** Position 1: a glass orb with a voice ring (docs/drawing.md, example 2). */
export function OrbDrawing() {
  return (
    <svg className="drawing drawing-orb" viewBox="0 0 100 100" aria-hidden="true">
      <circle cx="50" cy="50" r="30" className="ink-stroke" strokeWidth="2.4" fill="none" />
      <path
        className="ink-stroke spin-slow"
        fill="none"
        strokeWidth="1.6"
        d="M80 50 C80 58 76 66 70 71 C64 77 57 80 50 80 C42 80 35 76 29 71 C24 66 20 58 20 50 C20 42 24 34 29 29 C35 24 42 20 50 20 C58 20 65 23 71 29 C76 34 80 42 80 50 Z"
        strokeDasharray="3 5"
      />
    </svg>
  );
}

/** Position 2: an eye that looks inward, and at the pointer (docs/drawing.md, example 6). */
export function EyeDrawing({ pupil }: { pupil?: Ref<SVGCircleElement> }) {
  return (
    <svg className="drawing drawing-eye" viewBox="0 0 100 100" aria-hidden="true">
      <ellipse className="eye-white" cx="50" cy="50" rx="30" ry="22" />
      <circle ref={pupil} className="eye-pupil" cx="50" cy="50" r="9" />
    </svg>
  );
}

/** Position 3: a dot that pulses with any voice (docs/drawing.md, example 1). */
export function DotDrawing() {
  return (
    <svg className="drawing" viewBox="0 0 100 100" aria-hidden="true">
      <circle className="ink-fill pulse" cx="50" cy="50" r="18" />
    </svg>
  );
}

/** Position 4: a vinyl record (docs/drawing.md, example 4). */
export function VinylDrawing() {
  return (
    <svg className="drawing" viewBox="0 0 100 100" aria-hidden="true">
      <g className="spin-record">
        <circle cx="50" cy="50" r="44" fill="#141414" />
        <g fill="none" stroke="#ffffff" strokeOpacity="0.08" strokeWidth="0.7">
          <circle cx="50" cy="50" r="38" />
          <circle cx="50" cy="50" r="33" />
          <circle cx="50" cy="50" r="28" />
          <circle cx="50" cy="50" r="23" />
        </g>
        <circle cx="50" cy="50" r="15" fill="#c8352b" />
        <path d="M50 35 A15 15 0 0 1 65 50" stroke="#f3c8a8" strokeWidth="3" fill="none" />
        <circle cx="50" cy="50" r="1.8" fill="#000" />
      </g>
    </svg>
  );
}

/** Position 6: a gauge (docs/drawing.md, example 5). */
export function GaugeDrawing() {
  return (
    <svg className="drawing" viewBox="0 0 100 100" aria-hidden="true">
      <path className="gauge-track" d="M26 74 A34 34 0 1 1 74 74" fill="none" strokeWidth="8" strokeLinecap="round" />
      <path className="ink-stroke gauge-value" d="M26 74 A34 34 0 1 1 74 74" fill="none" strokeWidth="8" strokeLinecap="round" pathLength="100" />
    </svg>
  );
}

/** Position 7: a monogram, set in SF Pro Rounded where available. */
export function MonogramDrawing({ letter }: { letter: string }) {
  return (
    <svg className="drawing" viewBox="0 0 100 100" aria-hidden="true">
      <text className="ink-fill monogram" x="50" y="52" textAnchor="middle" dominantBaseline="central">
        {letter}
      </text>
    </svg>
  );
}

const RAYS = Array.from({ length: 10 }, (_, i) => (i * 360) / 10);

/** Position 8: a small sun whose rays turn slowly. */
export function SunDrawing() {
  return (
    <svg className="drawing" viewBox="0 0 100 100" aria-hidden="true">
      <circle cx="50" cy="50" r="14" fill="#ffd27a" />
      <g className="spin-slow" stroke="#ffd27a" strokeWidth="3.2" strokeLinecap="round">
        {RAYS.map((deg) => (
          <line key={deg} x1="50" y1="26" x2="50" y2="31" transform={`rotate(${deg} 50 50)`} />
        ))}
      </g>
    </svg>
  );
}
