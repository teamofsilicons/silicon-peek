// Reads ../docs/*.md at build time and prepares each page for components/docs/Markdown.tsx:
// heading ids (the same ids the previous prerenderer produced), the table of contents, rewritten
// links, and the plain text for search.json. Runs only on the server during `next build`.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { Marked, type Token, type Tokens } from "marked";
import { DOCS_DIR, describe, docsRoute, inlineText, lex, loadManifest, rewriteHref, slugify, splitTitle } from "./docs-core.mjs";

export type Heading = { id: string; text: string };
export type DocPage = {
  slug: string;
  title: string;
  group: string;
  summary: string;
  groupTitle: string;
  groupKind: string;
  description: string;
  markdown: string;
  tokens: Token[];
  toc: Heading[];
  headings: Heading[];
  text: string;
};
export type HeadingToken = Tokens.Heading & { id: string };

const inline = new Marked({ gfm: true });

function rewriteLinks(tokens: Token[] | undefined) {
  for (const token of tokens ?? []) {
    if (token.type === "link") (token as Tokens.Link).href = rewriteHref((token as Tokens.Link).href);
    if ("tokens" in token) rewriteLinks(token.tokens as Token[] | undefined);
    if (token.type === "list") for (const item of (token as Tokens.List).items) rewriteLinks(item.tokens);
    if (token.type === "table") {
      const table = token as Tokens.Table;
      for (const cell of table.header) rewriteLinks(cell.tokens);
      for (const row of table.rows) for (const cell of row) rewriteLinks(cell.tokens);
    }
  }
}

function prepare(body: string) {
  const tokens = lex(body) as Token[];
  rewriteLinks(tokens);
  const toc: Heading[] = [];
  const headings: Heading[] = [];
  const seen = new Map<string, number>();
  const visit = (list: Token[]) => {
    for (const token of list) {
      if (token.type === "heading") {
        const heading = token as HeadingToken;
        const stem = slugify(inline.parseInline(heading.text, { async: false })) || "section";
        const n = seen.get(stem) ?? 0;
        seen.set(stem, n + 1);
        heading.id = n ? `${stem}-${n}` : stem;
        const text = inlineText(heading.text);
        if (heading.depth === 2) toc.push({ id: heading.id, text });
        if (heading.depth === 2 || heading.depth === 3) headings.push({ id: heading.id, text });
      } else if (token.type === "list") {
        for (const item of (token as Tokens.List).items) visit(item.tokens);
      } else if (token.type === "blockquote") {
        visit((token as Tokens.Blockquote).tokens);
      }
    }
  };
  visit(tokens);
  const text = inline
    .parse(body, { async: false })
    .replace(/<[^>]*>/g, " ")
    .replace(/&[a-z]+;|&#\d+;/g, " ")
    .replace(/\s+/g, " ")
    .trim();
  return { tokens, toc, headings, text };
}

let cache: DocPage[] | null = null;

/** Every docs page: the index first, then the topics in manifest order. */
export function loadDocs(): DocPage[] {
  if (cache) return cache;
  const manifest = loadManifest();
  const groups = new Map<string, { title: string; kind: string }>(manifest.groups.map((g: { id: string; title: string; kind: string }) => [g.id, g]));
  cache = [manifest.index, ...manifest.topics].map((entry: { slug: string; title: string; group?: string; summary: string }) => {
    const source = join(DOCS_DIR, `${entry.slug}.md`);
    let markdown: string;
    try {
      markdown = readFileSync(source, "utf8");
    } catch (error) {
      throw new Error(`docs: missing ${source} for topic "${entry.slug}" (${(error as NodeJS.ErrnoException).code}). Add the page or remove it from topics.json.`);
    }
    const { title, body } = splitTitle(markdown, `docs/${entry.slug}.md`);
    if (title !== entry.title) throw new Error(`docs: docs/${entry.slug}.md is titled "${title}" but topics.json says "${entry.title}". Make them match.`);
    const group = entry.group ? groups.get(entry.group)! : { title: "Overview", kind: "Start" };
    return {
      slug: entry.slug,
      title,
      group: entry.group ?? "",
      summary: entry.summary,
      groupTitle: group.title,
      groupKind: group.kind,
      description: describe(body) || entry.summary,
      markdown,
      ...prepare(body),
    };
  });
  return cache;
}

export function getDoc(slug: string): { page: DocPage; previous?: DocPage; next?: DocPage } {
  const pages = loadDocs();
  const i = pages.findIndex((p) => p.slug === slug);
  if (i < 0) throw new Error(`docs: no page "${slug}"`);
  return { page: pages[i], previous: pages[i - 1], next: pages[i + 1] };
}

export { docsRoute };
