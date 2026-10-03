import { render } from "solid-js/web";
import "@fontsource/ibm-plex-mono/400.css";
import "../styles/tokens.css";
import "../styles/landing.css";
import "../styles/arc-controls.css";
import { startTelemetry } from "../shared/telemetry.ts";
import App from "./App.tsx";

startTelemetry();
render(() => <App />, document.getElementById("root")!);
