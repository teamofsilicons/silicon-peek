import { createSpaceStationWeb, type SpaceStationWeb } from "@teamofsilicons/space-station-web";
import { createSignal } from "solid-js";
import { telemetryTransport } from "./transport.ts";

/** localStorage key for this browser's choice: "off" opts out; anything else (or nothing) is on. */
export const PREFERENCE_KEY = "peek.telemetry";
export const ENDPOINT = "/api/web/telemetry";

// Only table *names* are baked into the bundle. The keys stay on the peek backend (docs/telemetry.md).
const analyticsTable = import.meta.env.VITE_PEEK_ANALYTICS_TABLE || "peekfrontendanalytics";
const eventsTable = import.meta.env.VITE_PEEK_EVENTS_TABLE || "peekfrontendevents";

function readPreference(): boolean {
  try {
    return globalThis.localStorage?.getItem(PREFERENCE_KEY) !== "off";
  } catch {
    return true;
  }
}

const [enabled, setEnabledSignal] = createSignal(readPreference());
let client: SpaceStationWeb | undefined;

/** Whether this browser shares anonymous usage (a Solid signal, so toggles re-render). */
export const telemetryEnabled = enabled;

/**
 * Starts browser telemetry once per page. Development builds never send anything, so local work
 * does not pollute the production tables.
 */
export function startTelemetry(): void {
  if (client || !import.meta.env.PROD || typeof window === "undefined") return;
  client = createSpaceStationWeb({
    analyticsTable,
    eventsTable,
    endpoint: ENDPOINT,
    fetch: telemetryTransport(window.fetch.bind(window), window.location.origin),
    enabled: enabled(),
  });
  window.addEventListener("pagehide", () => void client?.flush());
}

export function setTelemetryEnabled(next: boolean): void {
  try {
    globalThis.localStorage?.setItem(PREFERENCE_KEY, next ? "on" : "off");
  } catch {
    // Storage can be unavailable (private mode, blocked site data). The choice then lasts for this page.
  }
  setEnabledSignal(next);
  client?.setEnabled(next);
}

/** Records an explicit event. Never pass typed text, queries or other content here. */
export function track(name: string, data?: Record<string, string | number | boolean>): void {
  if (enabled()) client?.track(name, data);
}
