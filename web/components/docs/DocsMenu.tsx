"use client";

import { useState, type ReactNode } from "react";
import { Drawer, DrawerContent, DrawerTrigger } from "@/components/silicon-ui/drawer/drawer";

/** The documentation menu on narrow screens: the sidebar's links in Silicon UI's drawer. */
export default function DocsMenu({ children }: { children: ReactNode }) {
  const [open, setOpen] = useState(false);
  return (
    <Drawer open={open} onOpenChange={setOpen}>
      <DrawerTrigger className="menu-button" aria-label="Documentation menu">
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <path d="M4 7h16M4 12h16M4 17h16" />
        </svg>
      </DrawerTrigger>
      <DrawerContent side="left" title="Documentation" className="docs-drawer" onClick={(event) => {
        if ((event.target as HTMLElement).closest("a")) setOpen(false);
      }}>
        {children}
      </DrawerContent>
    </Drawer>
  );
}
