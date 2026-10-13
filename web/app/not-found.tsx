import { EmptyState } from "@/components/silicon-ui/empty-state/empty-state";
import { Footer, Header } from "@/components/peek/SiteChrome";
import { docsHref } from "@/lib/site";
import "@/styles/landing.css";

// The static export writes this page to 404.html, which Vercel serves (noindex, status 404) for
// every unknown address, under /docs or anywhere else.
export default function NotFound() {
  return (
    <>
      <title>Page not found · peek</title>
      <a className="skip-link" href="#main">
        Skip to content
      </a>
      <Header />
      <main id="main" className="not-found" tabIndex={-1}>
        <h1 className="sr-only">Page not found</h1>
        <div className="glass-panel not-found-card">
          <p className="kicker">404</p>
          <EmptyState
            title="This bubble slid away."
            description="There is nothing at this address. The documentation and the install command are one click away."
            action={
              <p className="not-found-links">
                <a href="/">Home</a>
                <a href={docsHref()}>Documentation</a>
                <a href="/install.sh">install.sh</a>
              </p>
            }
          />
        </div>
      </main>
      <Footer />
    </>
  );
}
