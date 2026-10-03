import { For, Show, createSignal, type JSX } from "solid-js";
import { copyText } from "../shared/copy.ts";
import { CRATE_URL, HOTKEY, INSTALL_COMMAND, REPO_URL, TOPICS, TOPIC_GROUPS, VERSION, docsHref } from "../shared/site.ts";
import TelemetryToggle from "../shared/TelemetryToggle.tsx";
import { track } from "../shared/telemetry.ts";
import Hero from "./Hero.tsx";

export function Mark(props: { size?: number }): JSX.Element {
  return (
    <svg class="mark" width={props.size ?? 26} height={props.size ?? 26} viewBox="0 0 64 64" aria-hidden="true">
      <rect width="64" height="64" rx="15" class="mark-bg" />
      <circle cx="32" cy="25" r="12.5" class="mark-fg" />
      <path d="M11.5 38.5 Q32 55 52.5 38.5" class="mark-arc" />
    </svg>
  );
}

function Header() {
  return (
    <header class="site-header">
      <nav class="site-nav" aria-label="Main">
        <a class="brand" href="/" aria-label="Peek home">
          <Mark />
          <span>peek</span>
        </a>
        <div class="nav-links">
          <a href="#carbons">For Carbons</a>
          <a href="#silicons">For Silicons</a>
          <a href={docsHref()}>Docs</a>
          <a href={REPO_URL} class="external">
            GitHub <span aria-hidden="true">↗</span>
          </a>
        </div>
        <a class="nav-cta" href="#install" onClick={() => track("download_cta", { place: "header" })}>
          Install
        </a>
      </nav>
    </header>
  );
}

function CodeBlock(props: { code: string; label: string; lang?: string }) {
  const [state, setState] = createSignal<"idle" | "copied" | "manual">("idle");
  let pre!: HTMLPreElement;
  return (
    <div class="code-block">
      <div class="code-head">
        <span>{props.label}</span>
        <button
          type="button"
          class="copy-mini"
          aria-label={`Copy: ${props.label}`}
          onClick={async () => {
            const ok = await copyText(props.code, pre);
            setState(ok ? "copied" : "manual");
            window.setTimeout(() => setState("idle"), 1800);
          }}
        >
          {state() === "copied" ? "Copied" : state() === "manual" ? "Selected" : "Copy"}
        </button>
      </div>
      <pre ref={pre} data-lang={props.lang}>
        <code>{props.code}</code>
      </pre>
    </div>
  );
}

function InstallSection() {
  const [copied, setCopied] = createSignal(false);
  let code!: HTMLElement;
  return (
    <section class="section install" id="install" aria-labelledby="install-title">
      <div class="section-head">
        <p class="kicker">Install</p>
        <h2 id="install-title">One command. Then Peek just shows up.</h2>
        <p>Run it once per Mac, as yourself. It never asks for a password and logs nobody in.</p>
      </div>
      <div class="install-card glass-panel">
        <code ref={code} class="install-big">{INSTALL_COMMAND}</code>
        <button
          type="button"
          class="copy-button large"
          data-spacestation-event="install_command_copied"
          onClick={async () => {
            track("install_command_copied", { place: "install" });
            setCopied(await copyText(INSTALL_COMMAND, code));
            window.setTimeout(() => setCopied(false), 2000);
          }}
        >
          {copied() ? "Copied" : "Copy command"}
        </button>
      </div>
      <ol class="steps">
        <li>
          <strong>Honeycomb</strong>
          <span>Installed if missing. Version 0.5.0 or newer is required.</span>
        </li>
        <li>
          <strong>The peek CLI</strong>
          <span>
            <code>honeycomb install 'peek'</code>. Updated automatically from then on.
          </span>
        </li>
        <li>
          <strong>Peek.app</strong>
          <span>
            Installed by the same command into ~/Applications, Developer ID signature verified, and started. It lives in the menu bar
            and updates itself.
          </span>
        </li>
        <li>
          <strong>No login</strong>
          <span>Silicons log in with their own short-lived IAM tokens. Carbons never log in.</span>
        </li>
      </ol>
      <p class="fine">
        Needs macOS 26 or newer on Apple silicon or Intel. On Linux and Windows, <code>honeycomb install 'peek'</code> installs
        the CLI for its IAM commands only. <a href={docsHref("platforms")}>Platforms</a>
      </p>
    </section>
  );
}

const KEYS: { keys: string[]; title: string; body: string }[] = [
  {
    keys: ["⌃", "⌘", "1–8"],
    title: "Open a Silicon's bubble",
    body: "Each Silicon owns one position: 1 is top centre, then clockwise. Prefer another modifier? Pick it in Settings.",
  },
  { keys: ["\\"], title: "Answer by voice", body: "Recorded only while you talk, transcribed once when you stop." },
  { keys: ["A–Z"], title: "Answer by typing", body: `Just start typing after ${HOTKEY.glyphs}N. Return sends it.` },
  {
    keys: ["↓"],
    title: "Close it",
    body: "One click slides it away, and dismisses a question. A double click also stops the speech.",
  },
  {
    keys: ["esc"],
    title: "Esc, right after it appears",
    body: "For 3 seconds, or while you hover it: once closes it, twice also stops the speech. A question folds up first, still answerable; ^ opens it again.",
  },
];

function CarbonsSection() {
  return (
    <section class="section carbons" id="carbons" aria-labelledby="carbons-title">
      <div class="section-head">
        <p class="kicker">For Carbons</p>
        <h2 id="carbons-title">Glance, answer, get back to work.</h2>
        <p>
          Bubbles slide in from the edge, say their piece and slide back on their own. When a Silicon asks something, click an option,
          or press its shortcut and speak or type. Hover over anything cut short to read all of it; click long text to open it in place.
          Each Silicon's bubbles take turns, so nothing is pushed away while you read; a small +N says how many more are waiting.
        </p>
      </div>
      <div class="key-grid">
        <For each={KEYS}>
          {(k) => (
            <article class="key-card glass-panel">
              <div class="keys" aria-hidden="true">
                <For each={k.keys}>{(key) => <kbd>{key}</kbd>}</For>
              </div>
              <h3>{k.title}</h3>
              <p>{k.body}</p>
            </article>
          )}
        </For>
        <article class="key-card glass-panel note">
          <h3>The microphone, only when you answer</h3>
          <p>
            The first time you answer by voice, macOS asks for microphone access. Once you stop, the recording goes through Peek
            to OpenAI to become text. The local recording is deleted after delivery. <a href={docsHref("privacy")}>Privacy</a>
          </p>
        </article>
      </div>
      <p class="section-link">
        <a href={docsHref("carbon")}>The full Carbon guide: settings, compact mode, Simulation →</a>
      </p>
    </section>
  );
}

const YAML = `silicon:
  id: si:dj
  # …
  apps:
    - peek   # Stemcell installs it and logs you in`;
const REGISTER = `# one position per Silicon (1 top, then clockwise)
peek register side 5
# your face on the Carbon's screen
peek register drawing ./cassette.js`;
const SEND = `peek send --speak "Found 3 GB of old builds." \\
  --ask '{"question":"Delete old builds?",
          "type":"single_choice",
          "options":["Delete","Keep"]}'
# → {"send_id":"snd_0192…","ask_id":"ask_0192…",…}`;
const TING = `{"type":"peek.ask.answered",
 "data":{"schema":1,"ask_id":"ask_0192…",
         "ask_type":"single_choice",
         "answer":{"kind":"single_choice",
                   "option_id":"1","label":"Delete"},
         "via":"voice","transcript":"delete them",
         "slot":5,"context":"production"},
 "metadata":{"isi":"deliberate",
             "peek_version":"${VERSION}"}}`;
const DRAWING = `let s = 0
peek.frame((ctx, input) => {
  const speech = input.speech?.level ?? 0
  const voice = Math.max(speech, input.mic.level)
  s += (voice - s) * Math.min(1, input.dt * 12)
  ctx.fillStyle = input.backdrop.ink
  ctx.beginPath()
  ctx.arc(50, 50, 18 + s * 14, 0, Math.PI * 2)
  ctx.fill()
  return s > 0.001
})`;

function SiliconsSection() {
  return (
    <section class="section silicons" id="silicons" aria-labelledby="silicons-title">
      <div class="section-head">
        <p class="kicker">For Silicons</p>
        <h2 id="silicons-title">Three commands and a flow branch.</h2>
        <p>
          <code>peek send</code> returns immediately, so it never blocks your turn. The Carbon's answer comes back later as a Ting
          event, carrying the ISI that asked in <code>metadata.isi</code>.
        </p>
      </div>
      <ol class="silicon-steps">
        <li>
          <h3>Add peek to silicon.yaml</h3>
          <p>
            Or from a running ISI: <code>si app install peek</code>.
          </p>
          <CodeBlock label="silicon.yaml" code={YAML} lang="yaml" />
        </li>
        <li>
          <h3>Claim a position and register a drawing</h3>
          <p>
            <code>peek send</code> refuses to run until both are set, and says exactly which command to run.
          </p>
          <CodeBlock label="once" code={REGISTER} lang="sh" />
        </li>
        <li>
          <h3>Speak, show or ask</h3>
          <p>
            Up to 2000 characters of speech, three show elements, or one question of at most 80 characters. Sends take turns on your
            position; give them a deadline with <code>--expires-in</code>, or schedule one with <code>--at</code> or <code>--in</code>.
          </p>
          <CodeBlock label="send" code={SEND} lang="sh" />
        </li>
        <li>
          <h3>Approve Ting, then receive the answer</h3>
          <p>
            Review Ting permission in IAM when prompted. After approval, answers are delivered at least once and retried while the Mac is offline. Route it to the asking ISI with the flow snippets in the docs.
          </p>
          <CodeBlock label="peek.ask.answered" code={TING} lang="json" />
        </li>
      </ol>
      <div class="drawing-callout glass-panel">
        <div>
          <h3>Your visual is a few lines of JavaScript</h3>
          <p>
            A canvas-style API in a fixed 100 × 100 square, plus Liquid Glass, blur and vibrancy. It reacts to speech, the Carbon's
            voice, the pointer and clicks, and sends nothing out.
          </p>
          <p>
            <a href={docsHref("drawing")}>Drawing the visual →</a>
          </p>
        </div>
        <CodeBlock label="pulse.js" code={DRAWING} lang="js" />
      </div>
      <p class="section-link">
        <a href={docsHref("silicon")}>The full Silicon guide: tools.md blurb, flows A and B, --wait →</a>
      </p>
    </section>
  );
}

const FACTS: { title: string; body: string; slug: string }[] = [
  { title: "Local first", body: "Bubbles, history and caches stay on the Mac.", slug: "privacy" },
  { title: "Voice only when you answer", body: "Recorded while you speak, then sent through Peek to OpenAI after you stop.", slug: "privacy" },
  { title: "Each Silicon keeps its own keys", body: "Ordinary sessions stay in each Silicon’s profile. Approved Ting permission is encrypted separately on the backend.", slug: "iam" },
  { title: "Delivered through Ting", body: "Answers wait in an outbox and retry until Ting accepts them.", slug: "ting" },
  { title: "Tested like production", body: "Testing environments use the same code path with isolated identities.", slug: "testing" },
  { title: "Open source", body: "Find a bug, patch it, and send the PR with peek report --pr.", slug: "development" },
];

function HowSection() {
  return (
    <section class="section how" id="how" aria-labelledby="how-title">
      <div class="section-head">
        <p class="kicker">Under the glass</p>
        <h2 id="how-title">A very local app that barely uses the network.</h2>
      </div>
      <div class="fact-grid">
        <For each={FACTS}>
          {(f) => (
            <a class="fact glass-panel" href={docsHref(f.slug)}>
              <h3>{f.title}</h3>
              <p>{f.body}</p>
            </a>
          )}
        </For>
      </div>
    </section>
  );
}

function DocsSection() {
  return (
    <section class="section docs-links" id="docs" aria-labelledby="docs-title">
      <div class="section-head">
        <p class="kicker">Documentation</p>
        <h2 id="docs-title">Instructions first, reasons one click away.</h2>
        <p>
          The same pages ship inside the CLI: <code>peek docs &lt;topic&gt;</code>. Agents can read them as Markdown from{" "}
          <a href="/llms.txt">llms.txt</a>.
        </p>
      </div>
      <div class="docs-columns">
        <For each={TOPIC_GROUPS}>
          {(group) => (
            <div class="docs-column">
              <h3>
                {group.title} <span>{group.kind}</span>
              </h3>
              <ul>
                <For each={TOPICS.filter((t) => t.group === group.id)}>
                  {(t) => (
                    <li>
                      <a href={docsHref(t.slug)}>
                        <strong>{t.title}</strong>
                        <span>{t.summary}</span>
                      </a>
                    </li>
                  )}
                </For>
              </ul>
            </div>
          )}
        </For>
      </div>
    </section>
  );
}

function Footer() {
  return (
    <footer class="site-footer">
      <div class="footer-inner">
        <div class="footer-brand">
          <a class="brand" href="/" aria-label="Peek home">
            <Mark />
            <span>peek</span>
          </a>
          <p>Glanceable, voice-first comms between Carbons and Silicons on macOS.</p>
        </div>
        <nav class="footer-links" aria-label="Footer">
          <div>
            <h2>Use</h2>
            <a href="#install">Install</a>
            <a href={docsHref("carbon")}>For Carbons</a>
            <a href={docsHref("silicon")}>For Silicons</a>
          </div>
          <div>
            <h2>Docs</h2>
            <a href={docsHref()}>All topics</a>
            <a href={docsHref("cli")}>CLI reference</a>
            <a href="/llms.txt">llms.txt</a>
          </div>
          <div>
            <h2>Source</h2>
            <a href={REPO_URL}>GitHub</a>
            <a href={CRATE_URL}>Rust crate</a>
            <a href={docsHref("development", "contribute")}>Report a bug</a>
          </div>
        </nav>
      </div>
      <div class="footer-bottom">
        <TelemetryToggle />
        <span class="footer-meta">
          peek {VERSION} · MIT · © 2026 Team of Silicons
        </span>
      </div>
    </footer>
  );
}

function NotFound() {
  return (
    <main class="not-found">
      <div class="glass-panel not-found-card">
        <p class="kicker">404</p>
        <h1>This bubble slid away.</h1>
        <p>There is nothing at this address. The documentation and the install command are one click away.</p>
        <p class="not-found-links">
          <a href="/">Home</a>
          <a href={docsHref()}>Documentation</a>
          <a href="/install.sh">install.sh</a>
        </p>
      </div>
    </main>
  );
}

export default function App() {
  const path = typeof location === "undefined" ? "/" : location.pathname;
  const home = path === "/" || path === "/index.html";
  return (
    <>
      <a class="skip-link" href="#main">
        Skip to content
      </a>
      <Header />
      <Show when={home} fallback={<NotFound />}>
        <main id="main" tabIndex={-1}>
          <Hero />
          <InstallSection />
          <CarbonsSection />
          <SiliconsSection />
          <HowSection />
          <DocsSection />
        </main>
      </Show>
      <Footer />
    </>
  );
}
