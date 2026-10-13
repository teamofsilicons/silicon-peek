// Verifies the exported site in dist/: every advertised topic exists with one H1, every internal link
// and anchor resolves (the landing page, the docs pages and the 404 page), each page's inline
// scripts are pinned by its CSP, and the search index, sitemap, llms.txt, Markdown copies and
// install.sh are complete. Exits non-zero with the full list of problems.
import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { readFile, readdir } from "node:fs/promises";
import { join } from "node:path";
import {
  DIST_DIR,
  DOCS_DIR,
  FORBIDDEN,
  REQUIRED_TOPICS,
  SITE_URL,
  WEB_DIR,
  docsFile,
  docsRoute,
  loadManifest,
  splitTitle,
} from "../lib/docs-core.mjs";
import { META_CSP, policyFor } from "./csp.mjs";

const problems = [];
const fail = (msg) => problems.push(msg);
const manifest = loadManifest();
const slugs = [manifest.index.slug, ...manifest.topics.map((t) => t.slug)];

// ------------------------------------------------------------------ sources
const manifestTopics = manifest.topics.map((t) => t.slug);
for (const topic of REQUIRED_TOPICS) {
  if (!manifestTopics.includes(topic)) fail(`topics.json: required topic "${topic}" is missing (peek docs advertises it)`);
}
for (const topic of manifestTopics) {
  if (!REQUIRED_TOPICS.includes(topic)) fail(`topics.json: "${topic}" is not one of the ${REQUIRED_TOPICS.length} peek docs topics`);
}
if (manifestTopics.join(",") !== REQUIRED_TOPICS.join(",")) fail("topics.json: topics must be in the order of `peek docs`");

const mdFiles = (await readdir(DOCS_DIR)).filter((f) => f.endsWith(".md"));
for (const file of mdFiles) {
  if (!slugs.includes(file.slice(0, -3))) fail(`docs/${file}: not listed in topics.json, so it would never be published`);
}
for (const slug of slugs) {
  const path = join(DOCS_DIR, `${slug}.md`);
  if (!existsSync(path)) {
    fail(`docs/${slug}.md: missing`);
    continue;
  }
  const md = await readFile(path, "utf8");
  try {
    splitTitle(md, `docs/${slug}.md`);
  } catch (error) {
    fail(error.message);
  }
  for (const { pattern, why } of FORBIDDEN) {
    if (pattern.test(md)) fail(`docs/${slug}.md: contains ${pattern} (${why})`);
  }
  // relative links written for GitHub must point at real docs
  for (const m of md.matchAll(/\]\((?:\.\/)?([a-z0-9-]+)\.md(?:#[^)]*)?\)/g)) {
    if (!slugs.includes(m[1])) fail(`docs/${slug}.md: links to ${m[1]}.md, which does not exist`);
  }
}

// ------------------------------------------------------------------ exported pages
const idsCache = new Map();
async function idsOf(file) {
  if (!idsCache.has(file)) {
    const html = await readFile(join(DIST_DIR, file), "utf8");
    idsCache.set(file, new Set([...html.matchAll(/\bid="([^"]+)"/g)].map((m) => m[1])));
  }
  return idsCache.get(file);
}

/** Maps an internal URL path to the dist file that serves it (vercel.json sets cleanUrls). */
function fileFor(path) {
  if (path === "/" || path === "/index.html") return "index.html";
  const clean = path.replace(/\/+$/, "");
  if (existsSync(join(DIST_DIR, `${clean}.html`))) return `${clean.slice(1)}.html`;
  return clean.slice(1);
}

let linksChecked = 0;
async function checkLink(from, url, ownIds) {
  if (url.startsWith("#")) {
    if (url.length > 1 && !ownIds.has(decodeURIComponent(url.slice(1)))) fail(`${from}: missing anchor ${url}`);
    linksChecked++;
    return;
  }
  if (!url.startsWith("/") || url.startsWith("//")) return;
  if (url.startsWith("/api/")) return; // proxied to the backend by vercel.json
  const [path, hash] = url.split("#");
  const file = fileFor(path.split("?")[0]);
  if (!file || !existsSync(join(DIST_DIR, file))) {
    fail(`${from}: broken link ${url}`);
    return;
  }
  if (hash && file.endsWith(".html") && !(await idsOf(file)).has(decodeURIComponent(hash))) {
    fail(`${from}: ${url} points at a missing anchor #${hash}`);
  }
  linksChecked++;
}

let scriptsPinned = 0;
const htmlFiles = [...slugs.map((s) => docsFile(s)), "404.html", "index.html"];
for (const file of htmlFiles) {
  const path = join(DIST_DIR, file);
  if (!existsSync(path)) {
    fail(`dist/${file}: not built`);
    continue;
  }
  const html = await readFile(path, "utf8");
  const ids = await idsOf(file);
  const allIds = [...html.matchAll(/\bid="([^"]+)"/g)].map((m) => m[1]);
  const dupes = allIds.filter((id, i) => allIds.indexOf(id) !== i);
  if (dupes.length) fail(`dist/${file}: duplicate ids ${[...new Set(dupes)].join(", ")}`);
  const h1 = (html.match(/<h1\b/g) || []).length;
  if (h1 !== 1) fail(`dist/${file}: expected exactly one <h1>, found ${h1}`);
  if (!/<link rel="stylesheet"[^>]+href="\/_next\/static\//.test(html)) fail(`dist/${file}: no stylesheet is linked`);
  const csp = META_CSP.exec(html);
  if (!csp) fail(`dist/${file}: no CSP <meta>; run scripts/csp.mjs after next build`);
  else if (csp[1] !== policyFor(html)) fail(`dist/${file}: the CSP <meta> does not match the page's inline scripts`);
  else if (html.indexOf(csp[0]) > html.indexOf("<script")) fail(`dist/${file}: the CSP <meta> must come before the first script`);
  else scriptsPinned += csp[1].split("'sha256-").length - 1;
  for (const m of html.matchAll(/\s(?:href|src)="([^"]+)"/g)) await checkLink(`dist/${file}`, m[1].replace(/&amp;/g, "&"), ids);
}

// links the pages build with docsHref("topic", "anchor")
async function sourceFiles(dir) {
  const out = [];
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) out.push(...(await sourceFiles(path)));
    else if (/\.(tsx?|mjs)$/.test(entry.name) && !entry.name.includes(".test.")) out.push(path);
  }
  return out;
}
let sourceLinks = 0;
for (const dir of ["app", "components", "lib"]) {
  for (const file of await sourceFiles(join(WEB_DIR, dir))) {
    const code = await readFile(file, "utf8");
    for (const m of code.matchAll(/docsHref\(\s*"([a-z0-9-]+)"(?:\s*,\s*"([a-z0-9-]+)")?\s*\)/g)) {
      await checkLink(file.slice(WEB_DIR.length + 1), docsRoute(m[1]) + (m[2] ? `#${m[2]}` : ""), new Set());
      sourceLinks++;
    }
  }
}
if (sourceLinks === 0) fail("sources: no docsHref() links found; expected the landing page to link to the docs");

// ------------------------------------------------------------------ generated files
const search = JSON.parse(await readFile(join(DIST_DIR, "search.json"), "utf8"));
for (const slug of slugs) {
  if (!search.some((e) => e.slug === slug && e.url === docsRoute(slug) && e.text.length > 200)) fail(`search.json: no usable entry for ${slug}`);
  const md = join(DIST_DIR, "docs", "markdown", `${slug}.md`);
  if (!existsSync(md)) fail(`dist/docs/markdown/${slug}.md: missing`);
  else if ((await readFile(md, "utf8")) !== (await readFile(join(DOCS_DIR, `${slug}.md`), "utf8"))) fail(`dist/docs/markdown/${slug}.md: differs from docs/${slug}.md`);
}
const sitemap = await readFile(join(DIST_DIR, "sitemap.xml"), "utf8");
for (const slug of slugs) if (!sitemap.includes(`<loc>${SITE_URL}${docsRoute(slug)}</loc>`)) fail(`sitemap.xml: missing ${docsRoute(slug)}`);
const llms = await readFile(join(DIST_DIR, "llms.txt"), "utf8");
for (const slug of slugs) if (!llms.includes(`${SITE_URL}/docs/markdown/${slug}.md`)) fail(`llms.txt: missing ${slug}`);
const robots = await readFile(join(DIST_DIR, "robots.txt"), "utf8");
if (!robots.includes(`Sitemap: ${SITE_URL}/sitemap.xml`)) fail("robots.txt: no Sitemap line");
if (!/<meta name="robots" content="noindex/.test(await readFile(join(DIST_DIR, "404.html"), "utf8"))) fail("404.html: must be noindex");

// ------------------------------------------------------------------ install.sh and vercel.json
const installPath = join(DIST_DIR, "install.sh");
if (!existsSync(installPath)) fail("dist/install.sh: missing");
else {
  const install = await readFile(installPath, "utf8");
  const source = await readFile(join(WEB_DIR, "public", "install.sh"), "utf8");
  if (install !== source) fail("dist/install.sh differs from public/install.sh");
  if (!install.startsWith("#!/bin/sh\n")) fail("install.sh: must start with #!/bin/sh");
  if (install.trimEnd().split("\n").pop() !== 'install_peek "$@"') fail('install.sh: the last line must be install_peek "$@" (so a partial download never runs)');
  try {
    execFileSync("/bin/sh", ["-n", installPath], { stdio: "pipe" });
  } catch (error) {
    fail(`install.sh: sh -n failed: ${String(error.stderr || error.message).trim()}`);
  }
}
const vercel = JSON.parse(await readFile(join(WEB_DIR, "vercel.json"), "utf8"));
if (vercel.outputDirectory !== "dist" || vercel.trailingSlash !== false || vercel.cleanUrls !== true) {
  fail("vercel.json: expected outputDirectory dist, cleanUrls true and trailingSlash false");
}
const header = vercel.headers.flatMap((h) => h.headers).find((h) => h.key === "Content-Security-Policy");
if (!header || !header.value.includes("script-src 'self'") || !header.value.includes("frame-ancestors 'none'")) fail("vercel.json: missing the CSP header");

// ------------------------------------------------------------------ report
if (problems.length) {
  console.error(`check-site: ${problems.length} problem(s):\n${problems.map((p) => `  - ${p}`).join("\n")}`);
  process.exit(1);
}
console.log(
  `check-site: ${slugs.length} docs pages (${REQUIRED_TOPICS.length} topics + index), landing, 404, ${linksChecked} internal links and anchors (${sourceLinks} from docsHref), ${scriptsPinned} pinned inline scripts, search, sitemap, llms.txt, install.sh: all good`,
);
