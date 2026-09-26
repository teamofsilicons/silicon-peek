// Shared helpers for build-docs.mjs and check-docs.mjs. Pure functions only, so they are unit-tested
// in docs-lib.test.mjs.
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { Lexer } from "marked";

export const WEB_DIR = join(dirname(fileURLToPath(import.meta.url)), "..");
export const DOCS_DIR = join(WEB_DIR, "..", "docs");
export const DIST_DIR = join(WEB_DIR, "dist");
export const SITE_URL = "https://peek.teamofsilicons.com";
export const REPO_URL = "https://github.com/teamofsilicons/silicon-peek";
export const VERSION = "0.1.0";

/** The topic manifest shared with the landing page (web/src/shared/topics.json). */
export function loadManifest() {
  const manifest = JSON.parse(readFileSync(join(WEB_DIR, "src", "shared", "topics.json"), "utf8"));
  const groups = new Map(manifest.groups.map((g) => [g.id, g]));
  for (const t of manifest.topics) {
    if (!groups.has(t.group)) throw new Error(`topics.json: topic ${t.slug} has unknown group ${t.group}`);
  }
  return manifest;
}

/** The 16 topics advertised by `peek docs` (BLUEPRINT §7.6), in order. */
export const REQUIRED_TOPICS = [
  "start",
  "carbon",
  "silicon",
  "cli",
  "show",
  "ask",
  "drawing",
  "ting",
  "iam",
  "testing",
  "telemetry",
  "privacy",
  "platforms",
  "development",
  "versioning",
  "troubleshooting",
];

/** Canonical route of a docs page. vercel.json sets trailingSlash: false, so no trailing slash. */
export function docsRoute(slug) {
  return slug === "index" ? "/docs" : `/docs/${slug}`;
}

/** Output file of a docs page, relative to dist/. */
export function docsFile(slug) {
  return slug === "index" ? "docs/index.html" : `docs/${slug}/index.html`;
}

/** Heading id: lowercase, runs of anything but a-z and 0-9 become "-". Same rule as the renderer. */
export function slugify(text) {
  return String(text)
    .toLowerCase()
    .replace(/<[^>]*>/g, "")
    .replace(/&[a-z]+;|&#\d+;/g, "")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

/**
 * Rewrites a link written for GitHub and `peek docs` (`ask.md`, `./cli.md#exit-codes`) into the
 * site route (`/docs/ask`, `/docs/cli#exit-codes`). Absolute URLs, anchors and other paths are kept.
 */
export function rewriteHref(href) {
  if (!href || href.startsWith("#") || href.startsWith("//") || /^[a-z][a-z0-9+.-]*:/i.test(href)) return href;
  const m = /^(?:\.\/)?([a-z0-9-]+)\.md(#[^\s]*)?$/.exec(href);
  if (m) return docsRoute(m[1]) + (m[2] ?? "");
  return href;
}

/** Top-level tokens of a Markdown document. */
export function lex(markdown) {
  return Lexer.lex(markdown, { gfm: true });
}

/** Splits a doc into its H1 title and the rest. Throws unless the doc starts with exactly one H1. */
export function splitTitle(markdown, name = "document") {
  const tokens = lex(markdown);
  const first = tokens.find((t) => t.type !== "space");
  if (!first || first.type !== "heading" || first.depth !== 1) {
    throw new Error(`${name}: must start with a single "# Title" line`);
  }
  const h1s = tokens.filter((t) => t.type === "heading" && t.depth === 1).length;
  if (h1s !== 1) throw new Error(`${name}: has ${h1s} H1 headings; exactly one is allowed`);
  const index = markdown.indexOf(first.raw);
  return { title: first.text.trim(), body: markdown.slice(index + first.raw.length) };
}

/** Plain text of inline Markdown (links keep their text, code keeps its content). */
export function inlineText(md) {
  return String(md)
    .replace(/`([^`]*)`/g, "$1")
    .replace(/!\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/[*_]{1,3}([^*_]+)[*_]{1,3}/g, "$1")
    .replace(/\s+/g, " ")
    .trim();
}

/** A one-paragraph description: the first paragraph, cut at a sentence end near 200 characters. */
export function describe(body) {
  const para = lex(body).find((t) => t.type === "paragraph");
  if (!para) return "";
  const text = inlineText(para.text);
  if (text.length <= 200) return text;
  const cut = text.slice(0, 200);
  const end = Math.max(cut.lastIndexOf(". "), cut.lastIndexOf("? "), cut.lastIndexOf(": "));
  return end > 80 ? cut.slice(0, end + 1) : `${cut.slice(0, cut.lastIndexOf(" "))}…`;
}

/** HTML escaping for text and attribute values. */
export function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
}

/** Words that must never appear in the docs (user directive: no live word or transcript data). */
export const FORBIDDEN = [
  { pattern: /speech\.word\b/, why: "speech.word was removed (no live word data)" },
  { pattern: /mic\.transcript\b/, why: "mic.transcript was removed (no live transcript)" },
  { pattern: /peek\.on\(\s*['"]word['"]/, why: "the word event was removed" },
];
