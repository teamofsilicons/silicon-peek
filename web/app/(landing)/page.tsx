import type { Metadata } from "next";
import Hero from "@/components/peek/Hero";
import { CarbonsSection, DocsSection, HowSection, InstallSection, SiliconsSection } from "@/components/peek/Sections";
import { Footer, Header } from "@/components/peek/SiteChrome";

const TITLE = "Peek · quick words between Carbons and Silicons";

export const metadata: Metadata = {
  title: TITLE,
  description:
    "Peek puts a small Liquid Glass bubble on your Mac so a Silicon can speak, show up to three things, or ask one quick question. Answer by voice, keyboard or click; answers return through Ting.",
  alternates: { canonical: "/", types: { "text/plain": [{ url: "/llms.txt", title: "llms.txt" }] } },
  openGraph: {
    type: "website",
    title: TITLE,
    description: "A voice-first glass bubble on your Mac for one-off messages between you and your Silicons.",
    url: "/",
  },
};

export default function Home() {
  return (
    <>
      <a className="skip-link" href="#main">
        Skip to content
      </a>
      <Header />
      <main id="main" tabIndex={-1}>
        <Hero />
        <InstallSection />
        <CarbonsSection />
        <SiliconsSection />
        <HowSection />
        <DocsSection />
      </main>
      <Footer />
    </>
  );
}
