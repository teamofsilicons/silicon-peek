# peek website and docs

The static site at https://peek.teamofsilicons.com: a SolidJS landing page with real CSS glass
bubbles, and the documentation prerendered from the repository's `../docs/*.md` into `/docs/<topic>`.
The static frontend uses the Peek backend for account sign-in through the `/api/*` proxy in `vercel.json`.

```sh
npm ci
npm run dev        # landing page at http://127.0.0.1:4317 (docs need a build)
npm run build      # tsc, vite build, prerender docs, then check every page and link
npm run preview    # the built site at http://127.0.0.1:4318, /docs included
```

What `npm run build` emits into `dist/`:

| Path | Source |
|---|---|
| `/` | `index.html` + `src/landing/*` (Solid SPA; `?demo=show\|speak\|ask\|slider` opens the hero demo in a state) |
| `/docs`, `/docs/<topic>` | `../docs/<topic>.md`, rendered by `scripts/build-docs.mjs` with `marked` into the shared docs shell; readable without JavaScript |
| `/docs/markdown/<topic>.md` | the Markdown sources, unchanged |
| `/search.json`, `/sitemap.xml`, `/robots.txt`, `/llms.txt`, `/404.html` | `scripts/build-docs.mjs` |
| `/install.sh` | `public/install.sh`, byte-identical to BLUEPRINT §9.4 (and `scripts/install.sh` at the repo root) |

`scripts/check-docs.mjs` fails the build when a `peek docs` topic is missing, a doc does not start
with exactly one H1, a page title and `src/shared/topics.json` disagree, an internal link or anchor
is broken (docs pages, the 404 page, and the links compiled into the landing bundle), a generated
file is incomplete, or `install.sh` does not parse.

Edit the docs in `../docs`, never in `dist`. Add a topic by creating `../docs/<topic>.md` and listing
it in `src/shared/topics.json` (the `peek docs` topic list in the CLI must match).

The website collects and sends no usage analytics or diagnostics.

Deploy with `./deploy.sh --prod` from this directory (or omit `--prod` for a preview). The script builds locally so the sibling `docs/` sources are available, then uploads Vercel's prebuilt output. The production project must already be linked in `.vercel/project.json`.
