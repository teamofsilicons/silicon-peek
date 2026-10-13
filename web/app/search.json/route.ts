import { loadDocs } from "@/lib/docs";
import { docsRoute } from "@/lib/docs-core.mjs";

// The docs search index, loaded by the search dialog on first open.
export const dynamic = "force-static";

export function GET() {
  return Response.json(
    loadDocs().map((page) => ({
      slug: page.slug,
      title: page.title,
      group: page.groupTitle,
      url: docsRoute(page.slug),
      description: page.description,
      headings: page.headings,
      text: page.text,
    })),
  );
}
