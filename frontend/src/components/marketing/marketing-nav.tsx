"use client";

import { Menu, X } from "lucide-react";
import Link from "next/link";
import { usePathname } from "next/navigation";
import { useEffect, useId, useRef, useState } from "react";

import { HEADER_LINKS, isCurrent } from "./site-links";

import { cn } from "@/lib/utils";

/**
 * The marketing site's main navigation. A client component only so it can mark
 * the current page and open the small-screen menu; the header around it stays a
 * server component.
 *
 * From `lg` up the links sit in the header row. Below it they do not fit beside
 * the wordmark, the theme toggle and the app button, so a Menu button discloses
 * them as a panel under the header. The current page is marked by weight and an
 * underline, and announced with `aria-current="page"`.
 */
export function MarketingNav() {
  const pathname = usePathname();
  const [open, setOpen] = useState(false);
  const panelId = useId();
  const root = useRef<HTMLDivElement>(null);
  const button = useRef<HTMLButtonElement>(null);

  // Close on navigation.
  useEffect(() => {
    setOpen(false);
  }, [pathname]);

  // Close on Escape (focus back to the button) and on a click outside.
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        setOpen(false);
        button.current?.focus();
      }
    };
    const onClick = (e: MouseEvent) => {
      if (root.current && !root.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("keydown", onKey);
    document.addEventListener("click", onClick);
    return () => {
      document.removeEventListener("keydown", onKey);
      document.removeEventListener("click", onClick);
    };
  }, [open]);

  const link = (href: string, label: string, mobile: boolean) => {
    const current = isCurrent(href, pathname);
    return (
      <Link
        key={href}
        href={href}
        aria-current={current ? "page" : undefined}
        data-testid={`marketing-${mobile ? "menu" : "nav"}-${label.toLowerCase().replace(/\s+/g, "-")}`}
        className={cn(
          "inline-flex min-h-11 items-center px-2 text-sm underline-offset-[6px] hover:underline",
          mobile && "w-full px-4 text-base",
          current && "font-semibold underline decoration-2",
        )}
      >
        {label}
      </Link>
    );
  };

  return (
    <div ref={root} className="ml-auto flex items-center">
      <nav aria-label="Main navigation" className="hidden items-center gap-4 lg:flex">
        {HEADER_LINKS.map((l) => link(l.href, l.label, false))}
      </nav>
      <button
        ref={button}
        type="button"
        className="focus-visible:ring-ring inline-flex min-h-11 items-center gap-2 rounded-md px-3 text-sm font-medium focus-visible:ring-2 focus-visible:outline-none lg:hidden"
        aria-expanded={open}
        aria-controls={panelId}
        onClick={() => {
          setOpen((v) => !v);
        }}
        data-testid="marketing-menu-button"
      >
        {open ? <X size={18} aria-hidden="true" /> : <Menu size={18} aria-hidden="true" />}
        Menu
      </button>
      <nav
        id={panelId}
        aria-label="Main navigation"
        hidden={!open}
        className="bg-background absolute inset-x-0 top-16 border-b py-2 shadow-sm lg:hidden"
        data-testid="marketing-menu-panel"
      >
        <ul className="mx-auto max-w-7xl px-2 sm:px-4">
          {HEADER_LINKS.map((l) => (
            <li key={l.href}>{link(l.href, l.label, true)}</li>
          ))}
          <li>{link("/", "Home", true)}</li>
        </ul>
      </nav>
    </div>
  );
}
