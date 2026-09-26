import { setTelemetryEnabled, telemetryEnabled } from "./telemetry.ts";
import { docsHref } from "./site.ts";

/** Footer switch that writes localStorage `peek.telemetry` and pauses or resumes the SDK. */
export default function TelemetryToggle() {
  return (
    <div class="telemetry-toggle">
      <button
        type="button"
        role="switch"
        class="switch"
        aria-checked={telemetryEnabled()}
        aria-describedby="telemetry-note"
        onClick={() => setTelemetryEnabled(!telemetryEnabled())}
      >
        <span class="switch-track" aria-hidden="true">
          <span class="switch-thumb" />
        </span>
        <span class="switch-label">Share anonymous usage</span>
      </button>
      <span id="telemetry-note" class="telemetry-note">
        {telemetryEnabled() ? "On" : "Off"} for this browser. Never includes what you type.{" "}
        <a href={docsHref("telemetry")}>What is recorded</a>
      </span>
    </div>
  );
}
