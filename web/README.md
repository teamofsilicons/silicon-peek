# peek website and docs

The site at https://peek.teamofsilicons.com: a Next.js (App Router) static export with the landing page, real CSS glass
bubbles, and the documentation rendered from the repository's `../docs/*.md` into `/docs/<topic>`. Interface components
come from [Silicon UI](https://ui.teamofsilicons.com); see [SILICON-UI.md](SILICON-UI.md). The site uses the Peek
backend for account sign-in through the `/api/*` proxy in `vercel.json`.

```sh
npm ci
npm run dev        # the whole site, docs included, at http://127.0.0.1:4317
npm run build      # next build (types included), pin each page's CSP, then check every page and link
npm run preview    # the built dist/ at http://127.0.0.1:4318, served like Vercel does (clean URLs, 404, headers)
```

What `npm run build` emits into `dist/`:

| Path | Source |
|---|---|
| `/` | `app/(landing)/page.tsx`, `components/peek/*` (`?demo=show\|speak\|ask\|slider` opens the hero demo in a state) |
| `/docs`, `/docs/<topic>` | `../docs/<topic>.md`, read by `lib/docs.ts` and rendered by `components/docs/*`; readable without JavaScript |
| `/docs/markdown/<topic>.md` | the Markdown sources, unchanged (`app/docs/markdown/[file]/route.ts`) |
| `/search.json`, `/sitemap.xml`, `/robots.txt`, `/llms.txt` | route handlers and metadata routes in `app/` |
| `/404.html` | `app/not-found.tsx` (noindex) |
| `/install.sh` | `public/install.sh`, byte-identical to BLUEPRINT §9.4 (and `scripts/install.sh` at the repo root) |

`scripts/csp.mjs` adds a `<meta>` Content-Security-Policy to every page that lists the SHA-256 of exactly its own
inline scripts (the App Router inlines its payload), so together with the header in `vercel.json` only those scripts and
same-origin files run. `scripts/check-site.mjs` then fails the build when a `peek docs` topic is missing, a doc does not
start with exactly one H1, a page title and `lib/topics.json` disagree, an internal link or anchor is broken, a page's
CSP does not match its scripts, a generated file is incomplete, or `install.sh` does not parse.

Edit the docs in `../docs`, never in `dist`. Add a topic by creating `../docs/<topic>.md` and listing it in
`lib/topics.json` (the `peek docs` topic list in the CLI must match).

The website collects and sends no usage analytics or diagnostics.

Deploy with `./deploy.sh --prod` from this directory (or omit `--prod` for a preview). The script builds locally so the
sibling `docs/` sources are available, then uploads Vercel's prebuilt output. The production project must already be
linked in `.vercel/project.json`.
