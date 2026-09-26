import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import { DOCS_DIR, FORBIDDEN, REQUIRED_TOPICS, describe, docsRoute, loadManifest, rewriteHref, slugify, splitTitle } from "./docs-lib.mjs";

test("rewriteHref turns doc links into site routes and leaves everything else alone", () => {
  assert.equal(rewriteHref("ask.md"), "/docs/ask");
  assert.equal(rewriteHref("./cli.md#exit-codes"), "/docs/cli#exit-codes");
  assert.equal(rewriteHref("index.md"), "/docs");
  assert.equal(rewriteHref("#local"), "#local");
  assert.equal(rewriteHref("https://example.com/a.md"), "https://example.com/a.md");
  assert.equal(rewriteHref("mailto:x@y.z"), "mailto:x@y.z");
  assert.equal(rewriteHref("/install.sh"), "/install.sh");
});

test("docsRoute has no trailing slash (vercel.json trailingSlash: false)", () => {
  assert.equal(docsRoute("index"), "/docs");
  assert.equal(docsRoute("platforms"), "/docs/platforms");
});

test("slugify matches GitHub-style heading anchors for plain headings", () => {
  assert.equal(slugify("Flow A: send every peek event to one ISI"), "flow-a-send-every-peek-event-to-one-isi");
  assert.equal(slugify("<code>peek iam</code>"), "peek-iam");
  assert.equal(slugify("Speak &amp; show"), "speak-show");
});

test("splitTitle requires exactly one leading H1 and ignores # inside code", () => {
  assert.deepEqual(splitTitle("# Title\n\nBody\n", "x").title, "Title");
  assert.throws(() => splitTitle("Intro\n# Title\n", "x"), /must start/);
  assert.throws(() => splitTitle("# A\n\n# B\n", "x"), /2 H1/);
  assert.equal(splitTitle("# A\n\n```sh\n# a comment\n```\n", "x").title, "A");
});

test("describe takes the first paragraph and keeps it short", () => {
  assert.equal(describe("\nFirst `code` and [a link](x.md).\n\nSecond."), "First code and a link.");
  const long = describe(`${"word ".repeat(30)}end. ${"more ".repeat(40)}`);
  assert.ok(long.length <= 201);
});

test("the manifest lists exactly the 16 peek docs topics, and every doc file is valid", () => {
  const manifest = loadManifest();
  assert.deepEqual(manifest.topics.map((t) => t.slug), REQUIRED_TOPICS);
  const files = readdirSync(DOCS_DIR).filter((f) => f.endsWith(".md")).sort();
  assert.deepEqual(files, [...REQUIRED_TOPICS, "index"].map((s) => `${s}.md`).sort());
  for (const entry of [manifest.index, ...manifest.topics]) {
    const md = readFileSync(join(DOCS_DIR, `${entry.slug}.md`), "utf8");
    assert.equal(splitTitle(md, entry.slug).title, entry.title, `${entry.slug}.md title`);
    for (const { pattern } of FORBIDDEN) assert.doesNotMatch(md, pattern, `${entry.slug}.md must not mention ${pattern}`);
  }
});
