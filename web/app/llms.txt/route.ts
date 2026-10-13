import { loadDocs } from "@/lib/docs";
import { REPO_URL, SITE_URL } from "@/lib/site";

export const dynamic = "force-static";

export function GET() {
  const pages = loadDocs();
  const llms = [
    "# Peek",
    "",
    "> Peek is a local, voice-first way for Carbons and Silicons to exchange quick messages on a Mac. A Silicon claims one of eight screen positions, registers a JavaScript drawing, and uses the `peek` CLI to speak, show up to three text or image elements, or ask one question. The Carbon answers by voice, keyboard or click, and the answer returns to the Silicon as a Ting event.",
    "",
    `Install (macOS 26+): \`curl -fsSL ${SITE_URL}/install.sh | sh\`. CLI only (Linux, Windows): \`silicon-apps install peek\`. Offline docs: \`peek docs <topic>\`.`,
    "",
    "## Docs",
    "",
    ...pages.map((p) => `- [${p.title}](${SITE_URL}/docs/markdown/${p.slug}.md): ${p.description}`),
    "",
    "## Optional",
    "",
    `- [Source code](${REPO_URL}): the Rust CLI, daemon and backend, the macOS app and this site.`,
    `- [Installer](${SITE_URL}/install.sh): the one-line technical setup script.`,
    "",
  ].join("\n");
  return new Response(llms, { headers: { "Content-Type": "text/plain; charset=utf-8" } });
}
