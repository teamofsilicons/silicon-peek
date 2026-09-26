import { For, Show, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { render } from "solid-js/web";
import "@fontsource/ibm-plex-mono/400.css";
import "../styles/tokens.css";
import "../styles/docs.css";
import { copyText } from "../shared/copy.ts";
import TelemetryToggle from "../shared/TelemetryToggle.tsx";
import { startTelemetry, track } from "../shared/telemetry.ts";

// Progressive enhancement for the prerendered docs: search (⌘K or /), copy buttons, the mobile
// menu and the telemetry switch. Every page is complete and readable without this script.

type Heading = { id: string; text: string };
type Entry = { slug: string; title: string; group: string; url: string; description: string; headings: Heading[]; text: string };
type Result = { entry: Entry; heading: Heading | null; score: number };

export function searchEntries(entries: Entry[], query: string, limit = 10): Result[] {
  const words = query.toLowerCase().trim().split(/\s+/).filter(Boolean);
  if (!words.length) return [];
  const results: Result[] = [];
  for (const entry of entries) {
    const title = entry.title.toLowerCase();
    const description = entry.description.toLowerCase();
    const headingText = entry.headings.map((h) => h.text.toLowerCase());
    const haystack = `${title} ${description} ${headingText.join(" ")} ${entry.text.toLowerCase()}`;
    if (!words.every((w) => haystack.includes(w))) continue;
    let best: Heading | null = null;
    let bestHits = 0;
    entry.headings.forEach((h, i) => {
      const hits = words.filter((w) => headingText[i].includes(w)).length;
      if (hits > bestHits) {
        best = h;
        bestHits = hits;
      }
    });
    const score = words.reduce(
      (n, w) => n + (title.includes(w) ? 10 : 0) + (description.includes(w) ? 3 : 0) + (headingText.some((h) => h.includes(w)) ? 5 : 0),
      0,
    );
    results.push({ entry, heading: best, score });
  }
  return results.sort((a, b) => b.score - a.score).slice(0, limit);
}

function Tools() {
  const [query, setQuery] = createSignal("");
  const [entries, setEntries] = createSignal<Entry[]>([]);
  const [state, setState] = createSignal<"idle" | "loading" | "ready" | "error">("idle");
  const [menu, setMenu] = createSignal(false);
  let dialog!: HTMLDialogElement;
  let input!: HTMLInputElement;
  let opener: HTMLElement | null = null;
  let trackTimer: number | undefined;

  const results = createMemo(() => searchEntries(entries(), query()));

  async function load() {
    setState("loading");
    try {
      const response = await fetch("/search.json");
      if (!response.ok) throw new Error(`search.json answered ${response.status}`);
      setEntries(await response.json());
      setState("ready");
    } catch {
      setState("error");
    }
  }

  function open() {
    opener = document.activeElement as HTMLElement | null;
    dialog.showModal();
    input.focus();
    input.select();
    if (state() === "idle" || state() === "error") void load();
  }

  function close() {
    if (dialog.open) dialog.close();
    opener?.focus();
  }

  function toggleMenu(next = !menu()) {
    setMenu(next);
    document.getElementById("sidebar")?.classList.toggle("is-open", next);
  }

  function onInput(value: string) {
    setQuery(value);
    clearTimeout(trackTimer);
    // Only the number of results is recorded, never the query itself.
    if (value.trim().length >= 2) trackTimer = window.setTimeout(() => track("docs_search", { results: results().length }), 1200);
  }

  onMount(() => {
    document.documentElement.classList.add("enhanced");
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      const typing = target?.closest("input, textarea, [contenteditable='true']");
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        if (dialog.open) close();
        else open();
      } else if (e.key === "/" && !typing && !dialog.open) {
        e.preventDefault();
        open();
      } else if (e.key === "Escape" && menu()) {
        toggleMenu(false);
      }
    };
    document.addEventListener("keydown", onKey);

    const removers: Array<() => void> = [];
    document.querySelectorAll<HTMLPreElement>("article pre").forEach((pre) => {
      const code = pre.querySelector("code");
      if (!code) return;
      const button = document.createElement("button");
      button.type = "button";
      button.className = "copy";
      button.textContent = "Copy";
      button.setAttribute("aria-label", "Copy code");
      const status = document.createElement("span");
      status.className = "sr-only";
      status.setAttribute("role", "status");
      button.addEventListener("click", async () => {
        const ok = await copyText(code.textContent ?? "", code);
        button.textContent = ok ? "Copied" : "Selected";
        status.textContent = ok ? "Code copied" : "Clipboard unavailable. The code is selected; copy it with your keyboard.";
        window.setTimeout(() => (button.textContent = "Copy"), 1800);
      });
      pre.prepend(button, status);
      removers.push(() => {
        button.remove();
        status.remove();
      });
    });

    document.querySelectorAll<HTMLAnchorElement>("#sidebar a").forEach((a) => a.addEventListener("click", () => toggleMenu(false)));

    onCleanup(() => {
      document.removeEventListener("keydown", onKey);
      removers.forEach((fn) => fn());
      clearTimeout(trackTimer);
    });
  });

  return (
    <>
      <button
        type="button"
        class="menu-button"
        aria-label="Documentation menu"
        aria-expanded={menu()}
        aria-controls="sidebar"
        onClick={() => toggleMenu()}
      >
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <path d="M4 7h16M4 12h16M4 17h16" />
        </svg>
      </button>
      <button type="button" class="search-button" aria-label="Search the documentation" aria-keyshortcuts="Meta+K /" onClick={open}>
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <circle cx="11" cy="11" r="6.5" />
          <path d="m16 16 4.5 4.5" />
        </svg>
        <span>Search the docs</span>
        <kbd>⌘K</kbd>
      </button>
      <dialog
        ref={dialog}
        class="search-dialog"
        aria-label="Search the documentation"
        onCancel={(e) => {
          e.preventDefault();
          close();
        }}
        onClick={(e) => {
          if (e.target === dialog) close();
        }}
      >
        <div class="search-inner">
          <div class="search-heading">
            <label for="docs-search">Search the docs</label>
            <button type="button" onClick={close} aria-label="Close search">
              Esc
            </button>
          </div>
          <input
            ref={input}
            id="docs-search"
            type="search"
            autocomplete="off"
            spellcheck={false}
            placeholder="Try keyterm, side_taken, --wait, telemetry…"
            value={query()}
            onInput={(e) => onInput(e.currentTarget.value)}
            onKeyDown={(e) => {
              if (e.key === "ArrowDown") {
                e.preventDefault();
                dialog.querySelector<HTMLAnchorElement>(".search-result")?.focus();
              }
            }}
          />
          <div class="search-results" aria-live="polite">
            <Show when={state() === "error"}>
              <p>
                Search could not load.{" "}
                <button type="button" class="text-button" onClick={load}>
                  Retry
                </button>
              </p>
            </Show>
            <Show when={state() === "loading"}>
              <p>Loading the index…</p>
            </Show>
            <Show when={state() === "ready" && query().trim() && !results().length}>
              <p>No results. Try “ask”, “login”, “drawing” or an error code.</p>
            </Show>
            <For each={results()}>
              {(r) => (
                <a
                  class="search-result"
                  href={r.heading ? `${r.entry.url}#${r.heading.id}` : r.entry.url}
                  onKeyDown={(e) => {
                    const links = [...dialog.querySelectorAll<HTMLAnchorElement>(".search-result")];
                    const i = links.indexOf(e.currentTarget);
                    if (e.key === "ArrowDown") {
                      e.preventDefault();
                      links[Math.min(links.length - 1, i + 1)]?.focus();
                    } else if (e.key === "ArrowUp") {
                      e.preventDefault();
                      if (i <= 0) input.focus();
                      else links[i - 1]?.focus();
                    }
                  }}
                >
                  <small>{r.entry.group}</small>
                  <strong>
                    {r.entry.title}
                    <Show when={r.heading}>{(h) => <span class="search-heading-hit"> › {h().text}</span>}</Show>
                  </strong>
                  <span>{r.entry.description}</span>
                </a>
              )}
            </For>
          </div>
          <div class="search-foot">
            <span>Search runs in your browser.</span>
            <span>↑ ↓ to move · Return to open</span>
          </div>
        </div>
      </dialog>
    </>
  );
}

startTelemetry();
const tools = document.getElementById("docs-tools");
if (tools) render(() => <Tools />, tools);
const toggle = document.getElementById("telemetry-toggle");
if (toggle) render(() => <TelemetryToggle />, toggle);
