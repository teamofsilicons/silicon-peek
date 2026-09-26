// Telemetry transport for @teamofsilicons/space-station-web, after silicon-ting/web/src/telemetry.ts.
// It keeps an upstream outage or an unrelated browser extension from generating a request storm:
// runtime errors from other origins and unhandled rejections (whose origin the SDK cannot tell)
// are dropped, and any failure pauses sending for 60 seconds.

type TelemetryEvent = { type: string; data?: { kind?: string; source?: string } };

export const COOL_DOWN_MS = 60_000;

export function isOwnEvent(event: TelemetryEvent, origin: string): boolean {
  if (event.type !== "error") return true;
  if (event.data?.kind === "unhandledrejection") return false;
  if (event.data?.kind !== "runtime") return true;
  try {
    return new URL(event.data.source || "").origin === origin;
  } catch {
    return false;
  }
}

export function telemetryTransport(
  send: typeof fetch,
  origin: string,
  now: () => number = Date.now,
): typeof fetch {
  let retryAfter = 0;
  return async (input, init) => {
    if (now() < retryAfter) throw new Error("Telemetry is cooling down after an upstream failure.");
    const body = JSON.parse(String(init?.body)) as { events: TelemetryEvent[] };
    body.events = body.events.filter((event) => isOwnEvent(event, origin));
    if (!body.events.length) return new Response(null, { status: 204 });
    try {
      const response = await send(input, { ...init, body: JSON.stringify(body) });
      if (!response.ok) retryAfter = now() + COOL_DOWN_MS;
      return response;
    } catch (error) {
      retryAfter = now() + COOL_DOWN_MS;
      throw error;
    }
  };
}
