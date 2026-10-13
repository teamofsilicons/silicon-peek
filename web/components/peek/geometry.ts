// Geometry of the eight peek positions, in the drawing's own conventions (y points down, angles in
// radians, positive is clockwise). Mirrors docs/drawing.md: `slot.facing` points from the bubble
// toward the centre of the screen, and the information arc sits on that inner side.

export type SlotIndex = 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8;
export type Side =
  | "top"
  | "top-right"
  | "right"
  | "bottom-right"
  | "bottom"
  | "bottom-left"
  | "left"
  | "top-left";

export type SlotGeometry = { index: SlotIndex; side: Side; facing: number };

const { PI } = Math;

export const SLOTS: readonly SlotGeometry[] = [
  { index: 1, side: "top", facing: PI / 2 },
  { index: 2, side: "top-right", facing: (3 * PI) / 4 },
  { index: 3, side: "right", facing: PI },
  { index: 4, side: "bottom-right", facing: (-3 * PI) / 4 },
  { index: 5, side: "bottom", facing: -PI / 2 },
  { index: 6, side: "bottom-left", facing: -PI / 4 },
  { index: 7, side: "left", facing: 0 },
  { index: 8, side: "top-left", facing: PI / 4 },
];

const round = (n: number) => Math.round(n * 1000) / 1000 + 0; // + 0 turns -0 into 0

/**
 * The centre of an information arc. peek's arc is flatter than a circle around the visual: its
 * centre sits `pivot` behind the visual (away from the screen centre), so the arc hugs the visual.
 */
export function arcCentre(facing: number, pivot: number): { x: number; y: number } {
  return { x: -Math.cos(facing) * pivot, y: -Math.sin(facing) * pivot };
}

/** An SVG arc of `radius`, centred on `facing`, spanning ±`spread` radians, around `arcCentre`. */
export function arcPath(facing: number, radius: number, spread: number, pivot = 0): string {
  const c = arcCentre(facing, pivot);
  const a0 = facing - spread;
  const a1 = facing + spread;
  const x0 = round(c.x + Math.cos(a0) * radius);
  const y0 = round(c.y + Math.sin(a0) * radius);
  const x1 = round(c.x + Math.cos(a1) * radius);
  const y1 = round(c.y + Math.sin(a1) * radius);
  const large = spread * 2 > PI ? 1 : 0;
  return `M ${x0} ${y0} A ${radius} ${radius} 0 ${large} 1 ${x1} ${y1}`;
}

/** Normalizes degrees to (-180, 180]. */
export function normalizeDegrees(deg: number): number {
  let d = deg % 360;
  if (d <= -180) d += 360;
  if (d > 180) d -= 360;
  return d + 0;
}

export type ArcPlacement = { x: number; y: number; rotate: number };

/**
 * Where an element sits on the information arc: `offset` radians from the arc's middle, at
 * `radius` from the arc's centre (see `arcCentre`). `rotate` (degrees) follows the arc's tangent but is kept
 * within ±90° so text is never upside down, exactly as peek rotates pills along the curve.
 */
export function arcPlacement(facing: number, radius: number, offset: number, pivot = 0): ArcPlacement {
  const c = arcCentre(facing, pivot);
  const angle = facing + offset;
  let rotate = normalizeDegrees(((angle + PI / 2) * 180) / PI);
  if (rotate > 90) rotate -= 180;
  if (rotate <= -90) rotate += 180;
  return {
    x: round(c.x + Math.cos(angle) * radius),
    y: round(c.y + Math.sin(angle) * radius),
    rotate: round(rotate),
  };
}

/** Arc-length based spacing: the angle an element of `width` occupies on an arc of `radius`. */
export function angularWidth(width: number, radius: number): number {
  return width / radius;
}

/** A point `offset` radians along an arc of `radius` around `arcCentre(facing, pivot)`. */
export function arcPoint(facing: number, radius: number, offset: number, pivot = 0): { x: number; y: number } {
  const c = arcCentre(facing, pivot);
  const a = facing + offset;
  return { x: c.x + Math.cos(a) * radius, y: c.y + Math.sin(a) * radius };
}

/** An SVG arc from offset `from` to offset `to` (radians, `from` < `to`), like `arcPath` but asymmetric. */
export function arcSpanPath(facing: number, radius: number, from: number, to: number, pivot = 0): string {
  const a = arcPoint(facing, radius, from, pivot);
  const b = arcPoint(facing, radius, Math.max(from, to), pivot);
  const large = Math.abs(to - from) > PI ? 1 : 0;
  return `M ${round(a.x)} ${round(a.y)} A ${radius} ${radius} 0 ${large} 1 ${round(b.x)} ${round(b.y)}`;
}

/**
 * Where a point falls along an arc span, as a fraction in 0…1 (clamped): 0 at offset `from`, 1 at
 * `to`. Used to drag a slider thumb along the curve: the thumb follows the pointer's angle.
 */
export function arcFraction(facing: number, pivot: number, from: number, to: number, point: { x: number; y: number }): number {
  const c = arcCentre(facing, pivot);
  const offset = (normalizeDegrees(((Math.atan2(point.y - c.y, point.x - c.x) - facing) * 180) / PI) * PI) / 180;
  return Math.min(1, Math.max(0, (offset - from) / (to - from)));
}

export type Box = { x: number; y: number; w: number; h: number };

/**
 * A curved band (a capsule bent along the arc) between radii `inner` and `outer`, spanning ±`spread`
 * around `facing`. Peek draws the waveform and the typing field this way, right under the question.
 * Returns the closed SVG path and its bounding box, both in the same units as the radii.
 */
export function curvedBand(facing: number, pivot: number, inner: number, outer: number, spread: number): { d: string; box: Box } {
  const cap = (outer - inner) / 2;
  const o0 = arcPoint(facing, outer, -spread, pivot);
  const o1 = arcPoint(facing, outer, spread, pivot);
  const i1 = arcPoint(facing, inner, spread, pivot);
  const i0 = arcPoint(facing, inner, -spread, pivot);
  const f = (p: { x: number; y: number }) => `${round(p.x)} ${round(p.y)}`;
  const d = `M ${f(o0)} A ${outer} ${outer} 0 0 1 ${f(o1)} A ${cap} ${cap} 0 0 1 ${f(i1)} A ${inner} ${inner} 0 0 0 ${f(i0)} A ${cap} ${cap} 0 0 1 ${f(o0)} Z`;
  // Bounds: sample both edges and the round caps.
  const pts: { x: number; y: number }[] = [];
  for (let i = 0; i <= 24; i++) {
    const t = -spread + (2 * spread * i) / 24;
    pts.push(arcPoint(facing, outer, t, pivot), arcPoint(facing, inner, t, pivot));
  }
  for (const end of [-spread, spread]) {
    const mid = arcPoint(facing, (inner + outer) / 2, end, pivot);
    for (let i = 0; i < 16; i++) {
      const a = (i / 16) * 2 * PI;
      pts.push({ x: mid.x + Math.cos(a) * cap, y: mid.y + Math.sin(a) * cap });
    }
  }
  const xs = pts.map((p) => p.x);
  const ys = pts.map((p) => p.y);
  const x = Math.min(...xs);
  const y = Math.min(...ys);
  return { d, box: { x: round(x), y: round(y), w: round(Math.max(...xs) - x), h: round(Math.max(...ys) - y) } };
}

/** Snaps `value` to the nearest step of a slider, within its bounds. */
export function snapToStep(value: number, min: number, max: number, step: number): number {
  const snapped = min + Math.round((value - min) / step) * step;
  return Math.min(max, Math.max(min, Math.round(snapped * 1e6) / 1e6));
}
