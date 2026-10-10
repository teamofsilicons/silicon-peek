// Prerenders ../docs/*.md into dist/docs/<topic>/index.html with the shared TOS docs shell, and
// emits search.json, sitemap.xml, robots.txt, 404.html, llms.txt and /docs/markdown/<topic>.md.
// Runs after `vite build`, which produced dist/docs-shell.html with the docs bundle's asset tags.
import { mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { Marked } from "marked";
import {
  DIST_DIR,
  DOCS_DIR,
  REPO_URL,
  SITE_URL,
  VERSION,
  describe,
  docsFile,
  docsRoute,
  escapeHtml,
  inlineText,
  loadManifest,
  rewriteHref,
  slugify,
  splitTitle,
} from "./docs-lib.mjs";

const manifest = loadManifest();
const groupsById = new Map(manifest.groups.map((g) => [g.id, g]));

// ------------------------------------------------------------------ asset tags from the docs entry
const shellPath = join(DIST_DIR, "docs-shell.html");
const shellHtml = await readFile(shellPath, "utf8");
const assetTags = [...shellHtml.matchAll(/<(?:script|link)\b[^>]*(?:src|href)="\/assets\/[^"<>]+"[^>]*>(?:<\/script>)?/g)]
  .map((m) => m[0])
  .join("");
if (!assetTags.includes("<script") || !assetTags.includes('rel="stylesheet"')) {
  throw new Error("build-docs: dist/docs-shell.html has no docs script or stylesheet; did `vite build` run with both entries?");
}
await rm(shellPath);

// ------------------------------------------------------------------ pages
const pages = [];
for (const entry of [manifest.index, ...manifest.topics]) {
  const source = join(DOCS_DIR, `${entry.slug}.md`);
  let markdown;
  try {
    markdown = await readFile(source, "utf8");
  } catch (error) {
    throw new Error(`build-docs: missing ${source} for topic "${entry.slug}" (${error.code}). Add the page or remove it from topics.json.`);
  }
  const { title, body } = splitTitle(markdown, `docs/${entry.slug}.md`);
  if (title !== entry.title) {
    throw new Error(`build-docs: docs/${entry.slug}.md is titled "${title}" but topics.json says "${entry.title}". Make them match.`);
  }
  const group = entry.group ? groupsById.get(entry.group) : { title: "Overview", kind: "Start" };
  pages.push({ ...entry, title, body, markdown, groupTitle: group.title, groupKind: group.kind, description: describe(body) || entry.summary });
}

function render(page) {
  const toc = [];
  const headings = [];
  const seen = new Map();
  const marked = new Marked({
    gfm: true,
    walkTokens(token) {
      if (token.type === "link") token.href = rewriteHref(token.href);
    },
    renderer: {
      heading({ tokens, depth, text }) {
        const html = this.parser.parseInline(tokens);
        const stem = slugify(html) || "section";
        const n = seen.get(stem) || 0;
        seen.set(stem, n + 1);
        const id = n ? `${stem}-${n}` : stem;
        const plain = inlineText(text);
        if (depth === 2) toc.push({ id, text: plain });
        if (depth === 2 || depth === 3) headings.push({ id, text: plain });
        return `<h${depth} id="${id}">${html}<a class="anchor" href="#${id}" aria-label="Link to ${escapeHtml(plain)}">#</a></h${depth}>\n`;
      },
      table(token) {
        // Default table rendering, wrapped so wide tables scroll instead of widening the page.
        const header = token.header.map((cell) => `<th${cell.align ? ` style="text-align:${cell.align}"` : ""}>${this.parser.parseInline(cell.tokens)}</th>`).join("");
        const rows = token.rows
          .map((row) => `<tr>${row.map((cell) => `<td${cell.align ? ` style="text-align:${cell.align}"` : ""}>${this.parser.parseInline(cell.tokens)}</td>`).join("")}</tr>`)
          .join("");
        return `<div class="table-wrap" tabindex="0" role="region" aria-label="Table"><table><thead><tr>${header}</tr></thead><tbody>${rows}</tbody></table></div>\n`;
      },
    },
  });
  const html = marked.parse(page.body);
  return { html, toc, headings };
}

const mark =
  '<svg class="mark" viewBox="0 0 64 64" width="28" height="28" aria-hidden="true"><rect width="64" height="64" rx="15" fill="#6c86c8"/><circle cx="32" cy="25" r="12.5" fill="#fff"/><path d="M11.5 38.5 Q32 55 52.5 38.5" fill="none" stroke="#fff" stroke-width="5.5" stroke-linecap="round"/></svg>';

function sidebar(current) {
  const link = (p) =>
    `<a href="${docsRoute(p.slug)}"${p.slug === current ? ' aria-current="page"' : ""}>${escapeHtml(p.title)}</a>`;
  const overview = pages.find((p) => p.slug === "index");
  const sections = manifest.groups
    .map(
      (g) =>
        `<section><h2>${escapeHtml(g.title)} <span>${escapeHtml(g.kind)}</span></h2>${pages
          .filter((p) => p.group === g.id)
          .map(link)
          .join("")}</section>`,
    )
    .join("");
  return `<nav id="sidebar" class="sidebar" aria-label="Documentation"><div class="edition">DOCUMENTATION <span>v${VERSION}</span></div><section><h2>Overview</h2>${link(overview)}</section>${sections}<a class="source-link" href="${REPO_URL}/tree/main/docs">Source on GitHub ↗</a></nav>`;
}

function shell({ page, article, toc, previous, next, notFound = false }) {
  const url = `${SITE_URL}${notFound ? "/404" : docsRoute(page.slug)}`;
  const title = notFound ? "Page not found · peek docs" : page.slug === "index" ? "peek documentation" : `${page.title} · peek docs`;
  const markdownPath = `/docs/markdown/${page.slug}.md`;
  const cliHint = page.slug === "index" ? "peek docs" : `peek docs ${page.slug}`;
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${escapeHtml(title)}</title>
<meta name="description" content="${escapeHtml(page.description)}">
${notFound ? '<meta name="robots" content="noindex">\n' : `<link rel="canonical" href="${url}">\n`}<meta name="theme-color" media="(prefers-color-scheme: light)" content="#fffdf9">
<meta name="theme-color" media="(prefers-color-scheme: dark)" content="#141516">
<meta name="color-scheme" content="light dark">
<meta property="og:type" content="article">
<meta property="og:title" content="${escapeHtml(title)}">
<meta property="og:description" content="${escapeHtml(page.description)}">
<meta property="og:url" content="${url}">
<link rel="icon" href="/favicon.svg" type="image/svg+xml">
${notFound ? "" : `<link rel="alternate" type="text/markdown" href="${markdownPath}" title="Markdown">\n`}${assetTags}
</head>
<body class="docs">
<a class="skip-link" href="#content">Skip to content</a>
<header class="topbar">
<a class="brand" href="/" aria-label="peek home">${mark}<strong>peek</strong><span>Docs</span></a>
<div id="docs-tools"></div>
<div id="docs-account"></div>
<nav class="external" aria-label="Site"><a href="/">Home</a><a href="${REPO_URL}">GitHub ↗</a><a class="install" href="/#install">Install</a></nav>
</header>
<div class="layout">
${sidebar(page.slug)}
<main id="content" tabindex="-1">
<div class="eyebrow">${escapeHtml(page.groupTitle)} <span>/</span> ${escapeHtml(page.groupKind)}</div>
<h1>${escapeHtml(page.title)}</h1>
<article>
${article}
</article>
<footer class="article-footer">
<div class="page-links">${previous ? `<a href="${docsRoute(previous.slug)}"><small>← PREVIOUS</small>${escapeHtml(previous.title)}</a>` : "<span></span>"}${next ? `<a class="next" href="${docsRoute(next.slug)}"><small>NEXT →</small>${escapeHtml(next.title)}</a>` : ""}</div>
<div class="meta"><span>peek ${VERSION} · MIT</span>${notFound ? "" : `<code>${escapeHtml(cliHint)}</code><a href="${markdownPath}">Read as Markdown</a><a href="${REPO_URL}/blob/main/docs/${page.slug}.md">View source ↗</a>`}</div>
<div id="telemetry-toggle" class="telemetry-slot"></div>
</footer>
</main>
<aside class="toc" aria-label="On this page">${toc.length ? `<h2>On this page</h2>${toc.map((h) => `<a href="#${h.id}">${escapeHtml(h.text)}</a>`).join("")}` : ""}<div class="toc-note">Instructive first.<br>Reasons one link away.</div></aside>
</div>
</body>
</html>
`;
}

const search = [];
await mkdir(join(DIST_DIR, "docs", "markdown"), { recursive: true });
const order = pages; // index first, then topics in manifest order
for (const [i, page] of order.entries()) {
  const { html, toc, headings } = render(page);
  const file = join(DIST_DIR, docsFile(page.slug));
  await mkdir(join(file, ".."), { recursive: true });
  await writeFile(file, shell({ page, article: html, toc, previous: order[i - 1], next: order[i + 1] }));
  await writeFile(join(DIST_DIR, "docs", "markdown", `${page.slug}.md`), page.markdown);
  search.push({
    slug: page.slug,
    title: page.title,
    group: page.groupTitle,
    url: docsRoute(page.slug),
    description: page.description,
    headings,
    text: html
      .replace(/<[^>]*>/g, " ")
      .replace(/&[a-z]+;|&#\d+;/g, " ")
      .replace(/\s+/g, " ")
      .trim(),
  });
}

await writeFile(join(DIST_DIR, "search.json"), JSON.stringify(search));

const urls = ["/", ...order.map((p) => docsRoute(p.slug))];
await writeFile(
  join(DIST_DIR, "sitemap.xml"),
  `<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n${urls
    .map((u) => `  <url><loc>${SITE_URL}${u === "/" ? "/" : u}</loc></url>`)
    .join("\n")}\n</urlset>\n`,
);
await writeFile(join(DIST_DIR, "robots.txt"), `User-agent: *\nAllow: /\nSitemap: ${SITE_URL}/sitemap.xml\n`);

const llms = [
  "# Peek",
  "",
  "> Peek is a local, voice-first way for Carbons and Silicons to exchange quick messages on a Mac. A Silicon claims one of eight screen positions, registers a JavaScript drawing, and uses the `peek` CLI to speak, show up to three text or image elements, or ask one question. The Carbon answers by voice, keyboard or click, and the answer returns to the Silicon as a Ting event.",
  "",
  `Install (macOS 26+): \`curl -fsSL ${SITE_URL}/install.sh | sh\`. CLI only (Linux, Windows): \`silicon-apps install peek\`. Offline docs: \`peek docs <topic>\`.`,
  "",
  "## Docs",
  "",
  ...order.map((p) => `- [${p.title}](${SITE_URL}/docs/markdown/${p.slug}.md): ${p.description}`),
  "",
  "## Optional",
  "",
  `- [Source code](${REPO_URL}): the Rust CLI, daemon and backend, the macOS app and this site.`,
  `- [Installer](${SITE_URL}/install.sh): the one-line technical setup script.`,
  "",
].join("\n");
await writeFile(join(DIST_DIR, "llms.txt"), llms);

const notFoundPage = {
  slug: "404",
  title: "This page slid away.",
  description: "There is nothing at this address. Find your way back to the peek documentation.",
  groupTitle: "Documentation",
  groupKind: "404",
};
await writeFile(
  join(DIST_DIR, "404.html"),
  shell({
    page: notFoundPage,
    article: `<p>Try the search above, go to the <a href="/docs">documentation home</a>, or start from <a href="/">the peek website</a>.</p><p>Looking for a CLI topic? Every page is also available offline with <code>peek docs &lt;topic&gt;</code>.</p>`,
    toc: [],
    notFound: true,
  }),
);

console.log(`build-docs: ${order.length} pages, search.json (${search.length} entries), Markdown copies, sitemap.xml, robots.txt, llms.txt, 404.html`);
