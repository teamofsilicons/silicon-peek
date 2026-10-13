import { loadDocs } from "@/lib/docs";

// /docs/markdown/<topic>.md: each page's Markdown source, unchanged, for agents and llms.txt.
export const dynamic = "force-static";
export const dynamicParams = false;

export function generateStaticParams() {
  return loadDocs().map((page) => ({ file: `${page.slug}.md` }));
}

export async function GET(_request: Request, { params }: { params: Promise<{ file: string }> }) {
  const { file } = await params;
  const page = loadDocs().find((p) => `${p.slug}.md` === file);
  if (!page) return new Response("Not found", { status: 404 });
  return new Response(page.markdown, { headers: { "Content-Type": "text/markdown; charset=utf-8" } });
}
