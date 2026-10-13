import type { Viewport } from "next";
import type { ReactNode } from "react";
import "@/styles/docs.css";

export const viewport: Viewport = {
  themeColor: [
    { media: "(prefers-color-scheme: light)", color: "#fffdf9" },
    { media: "(prefers-color-scheme: dark)", color: "#141516" },
  ],
};

export default function DocsLayout({ children }: { children: ReactNode }) {
  return children;
}
