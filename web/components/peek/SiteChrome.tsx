import { SiteFooter } from "@/components/silicon-ui/blocks/site-footer/site-footer";
import { SiteHeader } from "@/components/silicon-ui/blocks/site-header/site-header";
import { CRATE_URL, REPO_URL, VERSION, docsHref } from "@/lib/site";
import AccountMenu from "./AccountMenu";
import Mark from "./Mark";

/** The landing page's header: Silicon UI's site header with peek's mark and the account menu. */
export function Header() {
  return (
    <SiteHeader
      className="site-header"
      variant="simple"
      brand={{ name: "peek", href: "/", mark: <Mark /> }}
      items={[
        { value: "carbons", label: "For Carbons", href: "/#carbons" },
        { value: "silicons", label: "For Silicons", href: "/#silicons" },
        { value: "docs", label: "Docs", href: docsHref() },
        { value: "github", label: "GitHub", href: REPO_URL },
      ]}
      accessory={<AccountMenu />}
      secondaryAction={null}
      primaryAction={{ label: "Install", href: "/#install" }}
    />
  );
}

/** The landing page's footer: Silicon UI's site footer with peek's links. */
export function Footer() {
  return (
    <SiteFooter
      className="site-footer"
      variant="columns"
      brand={{ name: "peek", href: "/", mark: <Mark /> }}
      tagline="Glanceable, voice-first comms between Carbons and Silicons on macOS."
      columns={[
        { title: "Use", links: [{ label: "Install", href: "/#install" }, { label: "For Carbons", href: docsHref("carbon") }, { label: "For Silicons", href: docsHref("silicon") }] },
        { title: "Docs", links: [{ label: "All topics", href: docsHref() }, { label: "CLI reference", href: docsHref("cli") }, { label: "llms.txt", href: "/llms.txt" }] },
        { title: "Source", links: [{ label: "GitHub", href: REPO_URL }, { label: "Rust crate", href: CRATE_URL }, { label: "Report a bug", href: docsHref("development") }] },
      ]}
      legal={[
        { label: `peek ${VERSION}`, href: `${REPO_URL}/releases` },
        { label: "MIT License", href: `${REPO_URL}/blob/main/LICENSE` },
      ]}
      socials={[]}
      newsletter={null}
      status={null}
    />
  );
}
