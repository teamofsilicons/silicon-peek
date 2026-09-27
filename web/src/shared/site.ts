import manifest from "./topics.json";

export const SITE_URL = "https://peek.teamofsilicons.com";
export const REPO_URL = "https://github.com/teamofsilicons/silicon-peek";
export const CRATE_URL = "https://crates.io/crates/silicon-peek-client";
export const INSTALL_COMMAND = "curl -fsSL https://peek.teamofsilicons.com/install.sh | sh";
export const VERSION = "0.1.2";

/**
 * The Carbon's default shortcut modifier (ctrl+cmd, so ⌃⌘1…⌃⌘8). Plain cmd+1…8 switches tabs in most
 * browsers and editors; the Carbon can pick another modifier in Peek's Settings.
 */
export const HOTKEY = { glyphs: "⌃⌘", text: "ctrl+cmd" } as const;

export type TopicGroup = { id: string; title: string; kind: string };
export type Topic = { slug: string; title: string; group: string; summary: string };

export const TOPIC_GROUPS: readonly TopicGroup[] = manifest.groups;
export const TOPICS: readonly Topic[] = manifest.topics;

/** The canonical docs route for a topic (vercel.json uses trailingSlash: false). */
export function docsHref(slug?: string, hash?: string): string {
  const base = slug && slug !== "index" ? `/docs/${slug}` : "/docs";
  return hash ? `${base}#${hash}` : base;
}
