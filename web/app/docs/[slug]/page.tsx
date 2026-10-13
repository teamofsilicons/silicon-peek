import { loadDocs } from "@/lib/docs";
import DocsPage, { docsMetadata } from "../DocsPage";

type Params = { params: Promise<{ slug: string }> };

export const dynamicParams = false;

export function generateStaticParams() {
  return loadDocs()
    .filter((page) => page.slug !== "index")
    .map((page) => ({ slug: page.slug }));
}

export async function generateMetadata({ params }: Params) {
  return docsMetadata((await params).slug);
}

export default async function DocsTopic({ params }: Params) {
  return <DocsPage slug={(await params).slug} />;
}
