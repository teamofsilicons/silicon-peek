import type { ReactNode } from "react";
import { Badge } from "@/components/silicon-ui/badge/badge";
import { Breadcrumb } from "@/components/silicon-ui/breadcrumb/breadcrumb";
import AccountMenu from "@/components/peek/AccountMenu";
import Mark from "@/components/peek/Mark";
import { loadDocs, type DocPage, type Heading } from "@/lib/docs";
import { docsRoute } from "@/lib/docs-core.mjs";
import { REPO_URL, TOPIC_GROUPS, VERSION } from "@/lib/site";
import DocsMenu from "./DocsMenu";
import DocsSearch from "./DocsSearch";

// The shared TOS docs shell: a sticky header with search and the account menu, the topic sidebar,
// the article, and the page's table of contents. Every page is complete without JavaScript.

function DocsNav({ current, id }: { current: string; id?: string }) {
  const pages = loadDocs();
  const link = (p: DocPage) => (
    <a key={p.slug} href={docsRoute(p.slug)} aria-current={p.slug === current ? "page" : undefined}>
      {p.title}
    </a>
  );
  return (
    <nav id={id} className="sidebar" aria-label="Documentation">
      <div className="edition">
        DOCUMENTATION <span>v{VERSION}</span>
      </div>
      <section>
        <h2>Overview</h2>
        {link(pages[0])}
      </section>
      {TOPIC_GROUPS.map((g) => (
        <section key={g.id}>
          <h2>
            {g.title} <span>{g.kind}</span>
          </h2>
          {pages.filter((p) => p.group === g.id).map(link)}
        </section>
      ))}
      <a className="source-link" href={`${REPO_URL}/tree/main/docs`}>
        Source on GitHub ↗
      </a>
    </nav>
  );
}

export default function DocsShell({
  slug,
  title,
  groupTitle,
  groupKind,
  toc = [],
  previous,
  next,
  children,
}: {
  slug: string;
  title: string;
  groupTitle: string;
  groupKind: string;
  toc?: Heading[];
  previous?: DocPage;
  next?: DocPage;
  children: ReactNode;
}) {
  const markdownPath = `/docs/markdown/${slug}.md`;
  const cliHint = slug === "index" ? "peek docs" : `peek docs ${slug}`;
  const crumbs = slug === "index" ? [{ label: "Documentation", href: "/docs" }, { label: groupTitle }] : [{ label: "Documentation", href: "/docs" }, { label: groupTitle }, { label: title }];
  return (
    <div className="docs">
      <a className="skip-link" href="#content">
        Skip to content
      </a>
      <header className="topbar">
        <a className="brand" href="/" aria-label="peek home">
          <Mark size={28} />
          <strong>peek</strong>
          <span>Docs</span>
        </a>
        <div id="docs-tools">
          <DocsMenu>
            <DocsNav current={slug} />
          </DocsMenu>
          <DocsSearch />
        </div>
        <div id="docs-account">
          <AccountMenu />
        </div>
        <nav className="external" aria-label="Site">
          <a href="/">Home</a>
          <a href={REPO_URL}>GitHub ↗</a>
          <a className="install" href="/#install">
            Install
          </a>
        </nav>
      </header>
      <div className="layout">
        <DocsNav current={slug} id="sidebar" />
        <main id="content" tabIndex={-1}>
          <div className="eyebrow">
            <Breadcrumb items={crumbs} />
            <Badge size="sm">{groupKind}</Badge>
          </div>
          <h1>{title}</h1>
          <article>{children}</article>
          <footer className="article-footer">
            <div className="page-links">
              {previous ? (
                <a href={docsRoute(previous.slug)}>
                  <small>← PREVIOUS</small>
                  {previous.title}
                </a>
              ) : (
                <span />
              )}
              {next ? (
                <a className="next" href={docsRoute(next.slug)}>
                  <small>NEXT →</small>
                  {next.title}
                </a>
              ) : null}
            </div>
            <div className="meta">
              <span>peek {VERSION} · MIT</span>
              <code>{cliHint}</code>
              <a href={markdownPath}>Read as Markdown</a>
              <a href={`${REPO_URL}/blob/main/docs/${slug}.md`}>View source ↗</a>
            </div>
          </footer>
        </main>
        <aside className="toc" aria-label="On this page">
          {toc.length ? (
            <>
              <h2>On this page</h2>
              {toc.map((h) => (
                <a key={h.id} href={`#${h.id}`}>
                  {h.text}
                </a>
              ))}
            </>
          ) : null}
          <div className="toc-note">
            Instructive first.
            <br />
            Reasons one link away.
          </div>
        </aside>
      </div>
    </div>
  );
}
