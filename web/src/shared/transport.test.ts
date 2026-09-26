import assert from "node:assert/strict";
import { test } from "node:test";
import { COOL_DOWN_MS, isOwnEvent, telemetryTransport } from "./transport.ts";

const origin = "https://peek.teamofsilicons.com";

test("keeps ordinary events and own-origin runtime errors, drops foreign errors", () => {
  assert.equal(isOwnEvent({ type: "page_view" }, origin), true);
  assert.equal(isOwnEvent({ type: "error", data: { kind: "runtime", source: `${origin}/assets/a.js` } }, origin), true);
  assert.equal(isOwnEvent({ type: "error", data: { kind: "runtime", source: "chrome-extension://x/y.js" } }, origin), false);
  assert.equal(isOwnEvent({ type: "error", data: { kind: "runtime", source: "not a url" } }, origin), false);
  assert.equal(isOwnEvent({ type: "error", data: { kind: "unhandledrejection" } }, origin), false);
});

test("answers 204 locally when every event was filtered out", async () => {
  let calls = 0;
  const send = (async () => {
    calls++;
    return new Response(null, { status: 204 });
  }) as typeof fetch;
  const transport = telemetryTransport(send, origin);
  const body = JSON.stringify({ table: "t", events: [{ type: "error", data: { kind: "unhandledrejection" } }] });
  const response = await transport("/api/web/telemetry", { method: "POST", body });
  assert.equal(response.status, 204);
  assert.equal(calls, 0);
});

test("cools down for 60 s after a failed response", async () => {
  let now = 1_000;
  let calls = 0;
  const send = (async () => {
    calls++;
    return new Response(null, { status: 503 });
  }) as typeof fetch;
  const transport = telemetryTransport(send, origin, () => now);
  const body = JSON.stringify({ table: "t", events: [{ type: "page_view" }] });
  assert.equal((await transport("/x", { method: "POST", body })).status, 503);
  await assert.rejects(() => transport("/x", { method: "POST", body }), /cooling down/);
  now += COOL_DOWN_MS + 1;
  assert.equal((await transport("/x", { method: "POST", body })).status, 503);
  assert.equal(calls, 2);
});

test("cools down after a network error and rethrows it", async () => {
  let now = 0;
  const send = (async () => {
    throw new TypeError("network down");
  }) as typeof fetch;
  const transport = telemetryTransport(send, origin, () => now);
  const body = JSON.stringify({ table: "t", events: [{ type: "click" }] });
  await assert.rejects(() => transport("/x", { method: "POST", body }), /network down/);
  await assert.rejects(() => transport("/x", { method: "POST", body }), /cooling down/);
});
