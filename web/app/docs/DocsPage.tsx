import type { Metadata } from "next";
import DocsShell from "@/components/docs/DocsShell";
import Markdown from "@/components/docs/Markdown";
import { getDoc } from "@/lib/docs";
import { docsRoute } from "@/lib/docs-core.mjs";

export function docsMetadata(slug: string): Metadata {
  const { page } = getDoc(slug);
  const title = slug === "index" ? "peek documentation" : `${page.title} · peek docs`;
  return {
    title,
    description: page.description,
    alternates: { canonical: docsRoute(slug), types: { "text/markdown": [{ url: `/docs/markdown/${slug}.md`, title: "Markdown" }] } },
    openGraph: { type: "article", title, description: page.description, url: docsRoute(slug) },
  };
}

/** One prerendered docs page: the shell around the rendered Markdown. */
export default function DocsPage({ slug }: { slug: string }) {
  const { page, previous, next } = getDoc(slug);
  return (
    <DocsShell slug={slug} title={page.title} groupTitle={page.groupTitle} groupKind={page.groupKind} toc={page.toc} previous={previous} next={next}>
      <Markdown tokens={page.tokens} />
    </DocsShell>
  );
}
