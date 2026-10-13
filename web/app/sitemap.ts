import type { MetadataRoute } from "next";
import { loadDocs } from "@/lib/docs";
import { docsRoute } from "@/lib/docs-core.mjs";
import { SITE_URL } from "@/lib/site";

export const dynamic = "force-static";

export default function sitemap(): MetadataRoute.Sitemap {
  return [{ url: `${SITE_URL}/` }, ...loadDocs().map((page) => ({ url: `${SITE_URL}${docsRoute(page.slug)}` }))];
}
