import { existsSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig, type Plugin } from "vite";
import solid from "vite-plugin-solid";

/**
 * `vite preview` falls back to the landing page for /docs/<topic>, while Vercel serves the
 * prerendered dist/docs/<topic>/index.html. This maps extension-less paths to a directory index
 * when one exists, so local previews behave like production.
 */
function directoryIndex(): Plugin {
  return {
    name: "peek-directory-index",
    configurePreviewServer(server) {
      const outDir = join(server.config.root, server.config.build.outDir);
      server.middlewares.use((req, _res, next) => {
        const [path, query] = (req.url ?? "/").split("?");
        if (!/\.[a-z0-9]+$/i.test(path) && path !== "/") {
          const candidate = join(path.replace(/\/+$/, ""), "index.html");
          if (existsSync(join(outDir, candidate))) req.url = candidate + (query ? `?${query}` : "");
        }
        next();
      });
    },
  };
}

// Two entries: the landing page (index.html) and the shared docs bundle. scripts/build-docs.mjs
// copies the docs entry's asset tags into every prerendered /docs page, then removes docs-shell.html.
export default defineConfig({
  plugins: [solid(), directoryIndex()],
  build: {
    target: "es2022",
    // Never inline assets as data: URIs; the CSP allows fonts only from 'self'.
    assetsInlineLimit: 0,
    rolldownOptions: {
      input: {
        main: fileURLToPath(new URL("./index.html", import.meta.url)),
        docs: fileURLToPath(new URL("./docs-shell.html", import.meta.url)),
      },
    },
  },
});
