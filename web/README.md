# peek website and docs

The static site at https://peek.teamofsilicons.com: a SolidJS landing page with real CSS glass
bubbles, and the documentation prerendered from the repository's `../docs/*.md` into `/docs/<topic>`.
It has no runtime backend; `/api/*` is proxied to the peek backend by `vercel.json` (telemetry only).

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

Telemetry uses `@teamofsilicons/space-station-web` against the same-origin `/api/web/telemetry`
gateway. Only table names are built in (`VITE_PEEK_ANALYTICS_TABLE`, `VITE_PEEK_EVENTS_TABLE`,
defaulting to `peekfrontendanalytics` and `peekfrontendevents`); keys stay on the backend. Development
builds never send events. The footer switch stores the reader's choice in `localStorage` as
`peek.telemetry`. The gateway needs no `Idempotency-Key` (events are deduplicated by id), accepts a
browser request only when its `Origin` is listed in the backend's `PEEK_WEB_ORIGINS`
(`https://peek.teamofsilicons.com` in production; add your preview origin there for local tests), and
answers `204` without forwarding anything when the table has no key on the backend, when telemetry is
off, so a quiet gateway is not proof that events arrived
(`../docs/telemetry.md`, "The gateway").

Deploying is a mutating step owned by the operator (BLUEPRINT §9.3):
`npx --yes vercel@60.0.1 deploy --prod --yes --build-env VITE_PEEK_ANALYTICS_TABLE=peekfrontendanalytics --build-env VITE_PEEK_EVENTS_TABLE=peekfrontendevents`.
