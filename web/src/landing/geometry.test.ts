import assert from "node:assert/strict";
import { test } from "node:test";
import { SLOTS, arcFraction, arcPath, arcPlacement, arcPoint, arcSpanPath, curvedBand, normalizeDegrees, snapToStep } from "./geometry.ts";

test("positions run clockwise from top centre and face the screen centre", () => {
  assert.deepEqual(
    SLOTS.map((s) => s.side),
    ["top", "top-right", "right", "bottom-right", "bottom", "bottom-left", "left", "top-left"],
  );
  // top faces down (+y), bottom faces up (-y), right faces left (-x)
  assert.equal(Math.round(Math.sin(SLOTS[0].facing)), 1);
  assert.equal(Math.round(Math.sin(SLOTS[4].facing)), -1);
  assert.equal(Math.round(Math.cos(SLOTS[2].facing)), -1);
});

test("an element in the middle of the bottom arc sits straight above, unrotated", () => {
  const p = arcPlacement(-Math.PI / 2, 2, 0);
  assert.deepEqual(p, { x: 0, y: -2, rotate: 0 });
});

test("elements follow the tangent and never turn upside down", () => {
  const left = arcPlacement(-Math.PI / 2, 1, -Math.PI / 6);
  assert.ok(left.x < 0 && left.y < 0);
  assert.equal(left.rotate, -30);
  const top = arcPlacement(Math.PI / 2, 1, Math.PI / 6); // top slot, arc below
  assert.ok(Math.abs(top.rotate) <= 90);
  assert.equal(top.rotate, 30);
  const side = arcPlacement(Math.PI, 1, 0); // right slot: tangent is vertical
  assert.equal(Math.abs(side.rotate), 90);
});

test("arcPath spans the requested angle on the inner side", () => {
  assert.equal(arcPath(-Math.PI / 2, 1, Math.PI / 2), "M -1 0 A 1 1 0 0 1 1 0");
  assert.match(arcPath(Math.PI / 2, 2.5, 0.6), /^M [-\d.]+ [-\d.]+ A 2.5 2.5 0 0 1 [-\d.]+ [-\d.]+$/);
});

test("normalizeDegrees", () => {
  assert.equal(normalizeDegrees(270), -90);
  assert.equal(normalizeDegrees(-270), 90);
  assert.equal(normalizeDegrees(180), 180);
  assert.equal(normalizeDegrees(-180), 180);
});

test("a pivoted arc passes just outside the visual, like the reference layout", () => {
  // bottom position: pivot 2.06 and radius 2.87 (visual diameters) put the arc's middle 0.81 above
  // the visual's centre, and a point 20° along it at (-0.98, -0.64), tilted -20°.
  assert.deepEqual(arcPlacement(-Math.PI / 2, 2.87, 0, 2.06), { x: 0, y: -0.81, rotate: 0 });
  const year = arcPlacement(-Math.PI / 2, 2.87, (-20 * Math.PI) / 180, 2.06);
  assert.equal(year.rotate, -20);
  assert.ok(Math.abs(year.x + 0.982) < 0.01 && Math.abs(year.y + 0.637) < 0.01);
  assert.equal(arcPath(-Math.PI / 2, 1, Math.PI / 2, 0.5), "M -1 0.5 A 1 1 0 0 1 1 0.5");
});

test("arcSpanPath and arcFraction agree on an asymmetric span", () => {
  const up = -Math.PI / 2;
  assert.equal(arcSpanPath(up, 1, -Math.PI / 2, 0), "M -1 0 A 1 1 0 0 1 0 -1");
  // a point straight above the centre is halfway along a symmetric span, the ends clamp
  assert.equal(arcFraction(up, 0, -0.5, 0.5, { x: 0, y: -3 }), 0.5);
  assert.equal(arcFraction(up, 0, -0.5, 0.5, { x: -5, y: 0 }), 0);
  assert.equal(arcFraction(up, 0, -0.5, 0.5, { x: 5, y: 0.1 }), 1);
  // with a pivot the angle is measured from the arc's own centre, not the visual
  const p = arcPoint(up, 2.87, 0.1, 1.9);
  assert.ok(Math.abs(arcFraction(up, 1.9, -0.3, 0.3, p) - 2 / 3) < 1e-9);
});

test("curvedBand is a closed capsule whose box contains its ends", () => {
  const band = curvedBand(-Math.PI / 2, 1.9, 3.1, 3.38, 0.36);
  assert.match(band.d, /^M .* Z$/);
  assert.ok(band.box.w > 2 && band.box.h > 0.28);
  const end = arcPoint(-Math.PI / 2, 3.24, 0.36, 1.9);
  assert.ok(end.x < band.box.x + band.box.w && end.y < band.box.y + band.box.h);
  assert.ok(Math.abs(band.box.y - (1.9 - 3.38)) < 1e-3, "the band's top is the outer arc's apex");
});

test("snapToStep rounds to the nearest step inside the bounds", () => {
  assert.equal(snapToStep(7.3, 0, 10, 2), 8);
  assert.equal(snapToStep(6.9, 0, 10, 2), 6);
  assert.equal(snapToStep(-3, 0, 10, 2), 0);
  assert.equal(snapToStep(11, 0, 10, 2), 10);
  assert.equal(snapToStep(0.3, 0, 1, 0.1), 0.3);
});
