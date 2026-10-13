// Serves dist/ the way Vercel does with vercel.json: clean URLs (/docs/ask → docs/ask.html), the
// 404 page with a 404 status, and the security headers. /api is not proxied, so sign-in needs the
// deployed site, where Vercel forwards /api requests to the backend.
import { createServer } from "node:http";
import { readFile, stat } from "node:fs/promises";
import { dirname, extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";

const WEB_DIR = join(dirname(fileURLToPath(import.meta.url)), "..");
const DIST_DIR = join(WEB_DIR, "dist");

const PORT = Number(process.env.PORT ?? 4318);
const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".txt": "text/plain; charset=utf-8",
  ".md": "text/markdown; charset=utf-8",
  ".xml": "application/xml; charset=utf-8",
  ".svg": "image/svg+xml",
  ".woff2": "font/woff2",
  ".woff": "font/woff",
  ".sh": "text/x-shellscript; charset=utf-8",
};
const vercel = JSON.parse(await readFile(join(WEB_DIR, "vercel.json"), "utf8"));
const headers = Object.fromEntries(vercel.headers.find((h) => h.source === "/(.*)").headers.map((h) => [h.key, h.value]));

async function file(path) {
  try {
    return (await stat(path)).isFile() ? path : null;
  } catch {
    return null;
  }
}

createServer(async (req, res) => {
  const url = new URL(req.url ?? "/", "http://localhost");
  const path = normalize(decodeURIComponent(url.pathname)).replace(/^(\.\.[/\\])+/, "");
  const base = join(DIST_DIR, path);
  const found = path === "/" ? join(DIST_DIR, "index.html") : (await file(`${base.replace(/\/+$/, "")}.html`)) ?? (await file(base));
  const target = found ?? join(DIST_DIR, "404.html");
  res.writeHead(found ? 200 : 404, { ...headers, "Content-Type": TYPES[extname(target)] ?? "application/octet-stream" });
  res.end(await readFile(target));
}).listen(PORT, "127.0.0.1", () => console.log(`peek web preview: http://127.0.0.1:${PORT}`));
