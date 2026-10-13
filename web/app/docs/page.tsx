import DocsPage, { docsMetadata } from "./DocsPage";

export const metadata = docsMetadata("index");

export default function DocsIndex() {
  return <DocsPage slug="index" />;
}
