// Verifies the built site: every advertised topic exists with one H1, every internal link and anchor
// resolves (docs pages, the 404 page and links compiled into the landing bundle), and the
// search index, sitemap, llms.txt, Markdown copies and install.sh are complete. Exits non-zero with
// the full list of problems.
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
} from "./docs-lib.mjs";

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
  if (!REQUIRED_TOPICS.includes(topic)) fail(`topics.json: "${topic}" is not one of the 16 peek docs topics`);
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

// ------------------------------------------------------------------ built pages
const idsCache = new Map();
async function idsOf(file) {
  if (!idsCache.has(file)) {
    const html = await readFile(join(DIST_DIR, file), "utf8");
    const ids = new Set([...html.matchAll(/\bid="([^"]+)"/g)].map((m) => m[1]));
    if (file === "index.html") {
      // The landing page is rendered by Solid, so its section ids live in the compiled templates of
      // the scripts index.html loads (Solid drops attribute quotes where it can: id=install).
      for (const m of html.matchAll(/<script[^>]+src="\/(assets\/[^"]+\.js)"/g)) {
        const code = await readFile(join(DIST_DIR, m[1]), "utf8");
        for (const t of code.matchAll(/\bid=(?:\\?"([a-z][\w-]*)\\?"|([a-z][\w-]*))/g)) ids.add(t[1] || t[2]);
      }
    }
    idsCache.set(file, ids);
  }
  return idsCache.get(file);
}

/** Maps an internal URL path to the dist file that serves it, or null. */
function fileFor(path) {
  if (path === "/" || path === "/index.html") return "index.html";
  if (path === "/docs" || path === "/docs/") return "docs/index.html";
  const docs = /^\/docs\/([a-z0-9-]+)\/?$/.exec(path);
  if (docs && docs[1] !== "markdown") return `docs/${docs[1]}/index.html`;
  return path.replace(/^\//, "");
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
  const file = fileFor(path);
  if (!file || !existsSync(join(DIST_DIR, file))) {
    fail(`${from}: broken link ${url}`);
    return;
  }
  if (hash && file.endsWith(".html") && !(await idsOf(file)).has(decodeURIComponent(hash))) {
    fail(`${from}: ${url} points at a missing anchor #${hash}`);
  }
  linksChecked++;
}

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
  if (file !== "index.html") {
    const h1 = (html.match(/<h1\b/g) || []).length;
    if (h1 !== 1) fail(`dist/${file}: expected exactly one <h1>, found ${h1}`);
    if (!/<link rel="stylesheet"[^>]+\/assets\//.test(html)) fail(`dist/${file}: the docs stylesheet is not linked`);
  }
  if (/<script(?![^>]*\bsrc=)[^>]*>/.test(html.replace(/<script type="application\/ld\+json">/g, ""))) {
    fail(`dist/${file}: inline <script> would be blocked by the CSP (script-src 'self')`);
  }
  for (const m of html.matchAll(/\s(?:href|src)="([^"]+)"/g)) await checkLink(`dist/${file}`, m[1].replace(/&amp;/g, "&"), ids);
}

// links compiled into the landing bundle (it is a SPA, so its links live in JavaScript)
const assets = await readdir(join(DIST_DIR, "assets"));
let landingLinks = 0;
for (const js of assets.filter((f) => f.endsWith(".js"))) {
  const code = await readFile(join(DIST_DIR, "assets", js), "utf8");
  for (const m of code.matchAll(/["'`](\/docs(?:\/[a-z0-9-]+)?(?:#[a-z0-9-]+)?)["'`]/g)) {
    await checkLink(`dist/assets/${js}`, m[1], new Set());
    landingLinks++;
  }
}
if (landingLinks === 0) fail("dist/assets: the landing bundle links to no docs page; expected links to /docs");
// links the landing builds at runtime with docsHref("topic", "anchor")
async function sourceFiles(dir) {
  const out = [];
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) out.push(...(await sourceFiles(path)));
    else if (/\.(tsx?|mjs)$/.test(entry.name) && !entry.name.includes(".test.")) out.push(path);
  }
  return out;
}
for (const file of await sourceFiles(join(WEB_DIR, "src"))) {
  const code = await readFile(file, "utf8");
  for (const m of code.matchAll(/docsHref\(\s*"([a-z0-9-]+)"(?:\s*,\s*"([a-z0-9-]+)")?\s*\)/g)) {
    await checkLink(file.slice(WEB_DIR.length + 1), docsRoute(m[1]) + (m[2] ? `#${m[2]}` : ""), new Set());
    landingLinks++;
  }
}
// docsHref() builds routes at runtime; make sure every topic it can produce exists
for (const t of manifest.topics) if (!existsSync(join(DIST_DIR, docsFile(t.slug)))) fail(`landing: /docs/${t.slug} is linked but not built`);

// ------------------------------------------------------------------ generated files
const search = JSON.parse(await readFile(join(DIST_DIR, "search.json"), "utf8"));
for (const slug of slugs) {
  if (!search.some((e) => e.slug === slug && e.url === docsRoute(slug) && e.text.length > 200)) fail(`search.json: no usable entry for ${slug}`);
  if (!existsSync(join(DIST_DIR, "docs", "markdown", `${slug}.md`))) fail(`dist/docs/markdown/${slug}.md: missing`);
}
const sitemap = await readFile(join(DIST_DIR, "sitemap.xml"), "utf8");
for (const slug of slugs) if (!sitemap.includes(`<loc>${SITE_URL}${docsRoute(slug)}</loc>`)) fail(`sitemap.xml: missing ${docsRoute(slug)}`);
const llms = await readFile(join(DIST_DIR, "llms.txt"), "utf8");
for (const slug of slugs) if (!llms.includes(`${SITE_URL}/docs/markdown/${slug}.md`)) fail(`llms.txt: missing ${slug}`);
const robots = await readFile(join(DIST_DIR, "robots.txt"), "utf8");
if (!robots.includes(`Sitemap: ${SITE_URL}/sitemap.xml`)) fail("robots.txt: no Sitemap line");
if (!(await readFile(join(DIST_DIR, "404.html"), "utf8")).includes('name="robots" content="noindex"')) fail("404.html: must be noindex");
if (existsSync(join(DIST_DIR, "docs-shell.html"))) fail("dist/docs-shell.html: the build-only entry was not removed");

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
if (vercel.outputDirectory !== "dist" || vercel.trailingSlash !== false) fail("vercel.json: expected outputDirectory dist and trailingSlash false");
const csp = vercel.headers.flatMap((h) => h.headers).find((h) => h.key === "Content-Security-Policy");
if (!csp || !csp.value.includes("script-src 'self'")) fail("vercel.json: missing the script-src 'self' CSP");

// ------------------------------------------------------------------ report
if (problems.length) {
  console.error(`check-docs: ${problems.length} problem(s):\n${problems.map((p) => `  - ${p}`).join("\n")}`);
  process.exit(1);
}
console.log(
  `check-docs: ${slugs.length} pages (${REQUIRED_TOPICS.length} topics + index), ${linksChecked} internal links and anchors, ${landingLinks} landing links, search, sitemap, llms.txt, install.sh: all good`,
);
