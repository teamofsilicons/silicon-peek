import type { Viewport } from "next";
import type { ReactNode } from "react";
import "@/styles/landing.css";

export const viewport: Viewport = {
  themeColor: [
    { media: "(prefers-color-scheme: light)", color: "#f4efe8" },
    { media: "(prefers-color-scheme: dark)", color: "#0b0d13" },
  ],
};

export default function LandingLayout({ children }: { children: ReactNode }) {
  return children;
}
