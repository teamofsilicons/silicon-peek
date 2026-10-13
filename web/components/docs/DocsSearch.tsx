"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import * as Dialog from "@radix-ui/react-dialog";
import { FileText, Hash } from "lucide-react";
import { CommandPalette, type CommandItem } from "@/components/silicon-ui/command-palette/command-palette";

// Docs search (⌘K or /): Silicon UI's command palette in a dialog, over /search.json. The index loads
// on first open and the search runs in the browser.

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

const pageItem = (entry: Entry): CommandItem => ({ id: entry.url, label: entry.title, description: entry.description, group: entry.group, icon: <FileText width={16} height={16} /> });

export default function DocsSearch() {
  const [open, setOpen] = useState(false);
  const [entries, setEntries] = useState<Entry[]>([]);
  const [state, setState] = useState<"idle" | "loading" | "ready" | "error">("idle");
  const loading = useRef(false);

  const load = useCallback(async () => {
    if (loading.current) return;
    loading.current = true;
    setState("loading");
    try {
      const response = await fetch("/search.json");
      if (!response.ok) throw new Error(`search.json answered ${response.status}`);
      setEntries(await response.json());
      setState("ready");
    } catch {
      setState("error");
    } finally {
      loading.current = false;
    }
  }, []);

  const show = useCallback(
    (next: boolean) => {
      setOpen(next);
      if (next && (state === "idle" || state === "error")) void load();
    },
    [load, state],
  );

  useEffect(() => {
    document.documentElement.classList.add("enhanced");
    const onKey = (e: KeyboardEvent) => {
      const typing = (e.target as HTMLElement | null)?.closest("input, textarea, [contenteditable='true']");
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        show(!open);
      } else if (e.key === "/" && !typing && !open) {
        e.preventDefault();
        show(true);
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [open, show]);

  // Empty query: every page. Otherwise the ranked full-text results, each pointing at its best heading.
  const filter = useCallback(
    (_items: CommandItem[], query: string): CommandItem[] => {
      if (!query) return entries.map(pageItem);
      return searchEntries(entries, query).map(({ entry, heading }) =>
        heading
          ? { id: `${entry.url}#${heading.id}`, label: `${entry.title} › ${heading.text}`, description: entry.description, group: entry.group, icon: <Hash width={16} height={16} /> }
          : pageItem(entry),
      );
    },
    [entries],
  );

  const emptyTitle = state === "error" ? "Search could not load" : state === "loading" ? "Loading the index…" : "No results";
  const emptyDescription = state === "error" ? "Close the search and open it again to retry." : state === "loading" ? "One moment." : "Try “ask”, “login”, “drawing” or an error code.";

  return (
    <Dialog.Root open={open} onOpenChange={show}>
      <Dialog.Trigger className="search-button" aria-label="Search the documentation" aria-keyshortcuts="Meta+K /">
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <circle cx="11" cy="11" r="6.5" />
          <path d="m16 16 4.5 4.5" />
        </svg>
        <span>Search the docs</span>
        <kbd>⌘K</kbd>
      </Dialog.Trigger>
      <Dialog.Portal>
        <Dialog.Overlay className="search-overlay" />
        <Dialog.Content className="search-dialog" aria-describedby={undefined}>
          <Dialog.Title className="sr-only">Search the documentation</Dialog.Title>
          <CommandPalette
            autoFocus
            label="Search the docs"
            placeholder="Try keyterm, side_taken, --wait, privacy…"
            items={entries.map(pageItem)}
            filter={filter}
            emptyTitle={emptyTitle}
            emptyDescription={emptyDescription}
            onSelect={(item) => {
              setOpen(false);
              window.location.assign(item.id);
            }}
            onClose={() => setOpen(false)}
          />
          <p className="search-foot">
            <span>Search runs in your browser.</span>
            <span>↑ ↓ to move · Return to open</span>
          </p>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
