import { Badge } from "@/components/silicon-ui/badge/badge";
import { Card } from "@/components/silicon-ui/card/card";
import { CodeBlock } from "@/components/silicon-ui/code-block/code-block";
import { CopyButton } from "@/components/silicon-ui/copy-button/copy-button";
import { HOTKEY, INSTALL_COMMAND, REPO_URL, TOPICS, TOPIC_GROUPS, VERSION, docsHref } from "@/lib/site";

export function InstallSection() {
  return (
    <section className="section install" id="install" aria-labelledby="install-title">
      <div className="section-head">
        <p className="kicker">Install</p>
        <h2 id="install-title">One command. Then Peek just shows up.</h2>
        <p>Run it once per Mac, as yourself. It never asks for a password and logs nobody in.</p>
      </div>
      <div className="install-card glass-panel">
        <code className="install-big">{INSTALL_COMMAND}</code>
        <CopyButton value={INSTALL_COMMAND} label="Copy command" />
      </div>
      <ol className="steps">
        <li>
          <strong>A verified native release</strong>
          <span>Downloads the latest GitHub release and checks its checksum and code signatures.</span>
        </li>
        <li>
          <strong>The peek CLI</strong>
          <span>
            Installed into <code>~/.local/bin</code>, ready for account sign-in and commands.
          </span>
        </li>
        <li>
          <strong>Peek.app</strong>
          <span>
            Installed into <code>~/Applications</code>, its Developer ID signature verified, and started in the menu bar.
          </span>
        </li>
        <li>
          <strong>Your own account</strong>
          <span>Carbons and Silicons use their own Silicon Accounts identity. Sessions stay signed in until they expire or you sign out.</span>
        </li>
      </ol>
      <p className="fine">
        Needs macOS 26 or newer on Apple silicon or Intel. Rerun this installer for native updates. On Linux, use{" "}
        <code>silicon-apps install peek</code>. Windows CLI binaries are available from <a href={`${REPO_URL}/releases`}>GitHub releases</a>.{" "}
        <a href={docsHref("platforms")}>Platforms</a>
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

const Keys = ({ keys }: { keys: string[] }) => (
  <span className="keys" aria-hidden="true">
    {keys.map((key) => (
      <kbd key={key}>{key}</kbd>
    ))}
  </span>
);

export function CarbonsSection() {
  return (
    <section className="section carbons" id="carbons" aria-labelledby="carbons-title">
      <div className="section-head">
        <p className="kicker">For Carbons</p>
        <h2 id="carbons-title">Glance, answer, get back to work.</h2>
        <p>
          Bubbles slide in from the edge, say their piece and slide back on their own. When a Silicon asks something, click an option,
          or press its shortcut and speak or type. Hover over anything cut short to read all of it; click long text to open it in place.
          Each Silicon&apos;s bubbles take turns, so nothing is pushed away while you read; a small +N says how many more are waiting.
        </p>
      </div>
      <div className="key-grid">
        {KEYS.map((k) => (
          <Card key={k.title} className="key-card" title={k.title} description={k.body} media={<Keys keys={k.keys} />} />
        ))}
        <Card
          className="key-card note"
          title="The microphone, only when you answer"
          description="The first time you answer by voice, macOS asks for microphone access. Once you stop, the recording goes through Peek to OpenAI to become text. The local recording is deleted after delivery."
          action={<a href={docsHref("privacy")}>Privacy →</a>}
        />
      </div>
      <p className="section-link">
        <a href={docsHref("carbon")}>The full Carbon guide: settings, compact mode, Simulation →</a>
      </p>
    </section>
  );
}

const INSTALL = `${INSTALL_COMMAND}
silicon-accounts login --app peek --json | jq -r .slt | peek login --token-file -`;
const REGISTER = `# one position per Silicon (1 top, then clockwise)
peek register side 5
# optional: customize your face on the Carbon's screen
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

export function SiliconsSection() {
  return (
    <section className="section silicons" id="silicons" aria-labelledby="silicons-title">
      <div className="section-head">
        <p className="kicker">For Silicons</p>
        <h2 id="silicons-title">Install, sign in, and send.</h2>
        <p>
          <code>peek send</code> returns immediately, so it never blocks your turn. The Carbon&apos;s answer comes back later as a Ting
          event, carrying the ISI that asked in <code>metadata.isi</code>.
        </p>
      </div>
      <ol className="silicon-steps">
        <li>
          <h3>Install Peek and sign in</h3>
          <p>On macOS, run the native installer, then use your own Silicon Accounts identity.</p>
          <CodeBlock filename="install and sign in" language="sh" code={INSTALL} />
        </li>
        <li>
          <h3>Choose a position</h3>
          <p>Register one of eight positions for your account. A custom drawing is optional and can be added later.</p>
          <CodeBlock filename="once" language="sh" code={REGISTER} />
        </li>
        <li>
          <h3>Speak, show or ask</h3>
          <p>
            Up to 2000 characters of speech, three show elements, or one question of at most 80 characters. Sends take turns on your
            position; give them a deadline with <code>--expires-in</code>, or schedule one with <code>--at</code> or <code>--in</code>.
          </p>
          <CodeBlock filename="send" language="sh" code={SEND} />
        </li>
        <li>
          <h3>Approve Ting, then receive the answer</h3>
          <p>
            Enable Ting deliveries for your account when prompted. Once enabled, answers are delivered at least once and retried while the
            Mac is offline. Route it to the asking ISI with the flow snippets in the docs.
          </p>
          <CodeBlock filename="peek.ask.answered" language="json" code={TING} />
        </li>
      </ol>
      <div className="drawing-callout glass-panel">
        <div>
          <h3>Your visual is a few lines of JavaScript</h3>
          <p>
            A canvas-style API in a fixed 100 × 100 square, plus Liquid Glass, blur and vibrancy. It reacts to speech, the Carbon&apos;s
            voice, the pointer and clicks, and sends nothing out.
          </p>
          <p>
            <a href={docsHref("drawing")}>Drawing the visual →</a>
          </p>
        </div>
        <CodeBlock filename="pulse.js" language="js" code={DRAWING} />
      </div>
      <p className="section-link">
        <a href={docsHref("silicon")}>The full Silicon guide: tools.md blurb, flows A and B, --wait →</a>
      </p>
    </section>
  );
}

const FACTS: { title: string; body: string; slug: string }[] = [
  { title: "Local first", body: "Bubbles, history and caches stay on the Mac.", slug: "privacy" },
  { title: "Voice only when you answer", body: "Recorded while you speak, then sent through Peek to OpenAI after you stop.", slug: "privacy" },
  { title: "Each Silicon keeps its own keys", body: "Ordinary sessions stay in each Silicon’s profile. Ting deliveries use verified permission for the same account.", slug: "accounts" },
  { title: "Delivered through Ting", body: "Answers wait in an outbox and retry until Ting accepts them.", slug: "ting" },
  { title: "Open source", body: "Find a bug, patch it, and send the PR with peek report --pr.", slug: "development" },
];

export function HowSection() {
  return (
    <section className="section how" id="how" aria-labelledby="how-title">
      <div className="section-head">
        <p className="kicker">Under the glass</p>
        <h2 id="how-title">A very local app that barely uses the network.</h2>
      </div>
      <div className="fact-grid">
        {FACTS.map((f) => (
          <Card key={f.title} className="fact" title={f.title} description={f.body} action={<a href={docsHref(f.slug)}>Read more →</a>} />
        ))}
      </div>
    </section>
  );
}

export function DocsSection() {
  return (
    <section className="section docs-links" id="docs" aria-labelledby="docs-title">
      <div className="section-head">
        <p className="kicker">Documentation</p>
        <h2 id="docs-title">Instructions first, reasons one click away.</h2>
        <p>
          The same pages ship inside the CLI: <code>peek docs &lt;topic&gt;</code>. Agents can read them as Markdown from{" "}
          <a href="/llms.txt">llms.txt</a>.
        </p>
      </div>
      <div className="docs-columns">
        {TOPIC_GROUPS.map((group) => (
          <div className="docs-column" key={group.id}>
            <h3>
              {group.title} <Badge size="sm">{group.kind}</Badge>
            </h3>
            <ul>
              {TOPICS.filter((t) => t.group === group.id).map((t) => (
                <li key={t.slug}>
                  <a href={docsHref(t.slug)}>
                    <strong>{t.title}</strong>
                    <span>{t.summary}</span>
                  </a>
                </li>
              ))}
            </ul>
          </div>
        ))}
      </div>
    </section>
  );
}
