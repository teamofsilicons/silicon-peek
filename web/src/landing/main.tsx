import { render } from "solid-js/web";
import "@fontsource/ibm-plex-mono/400.css";
import "../components/silicon-ui/foundation.css";
import "../styles/tokens.css";
import "../shared/theme.ts";
import "../styles/landing.css";
import "../styles/arc-controls.css";
import App from "./App.tsx";

render(() => <App />, document.getElementById("root")!);
