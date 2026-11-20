"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";

import { cn } from "@/lib/utils";

/**
 * The marketing site's main navigation. A client component only so it can mark
 * the current page; the header around it stays a server component.
 *
 * Hand-maintained, as the landing-page skill notes: nothing derives these links
 * from MARKETING_PATHS. The current page is marked by weight and an underline,
 * and announced with `aria-current="page"`.
 */
const LINKS = [
  { href: "/", label: "Home", hideOnPhone: true },
  { href: "/docs", label: "Docs", hideOnPhone: false },
  { href: "/pricing", label: "Pricing", hideOnPhone: false },
] as const;

export function MarketingNav() {
  const pathname = usePathname();
  return (
    <nav aria-label="Main navigation" className="ml-auto flex items-center gap-1 sm:gap-5">
      {LINKS.map(({ href, label, hideOnPhone }) => {
        const current = href === "/" ? pathname === "/" : pathname.startsWith(href);
        return (
          <Link
            key={href}
            href={href}
            aria-current={current ? "page" : undefined}
            data-testid={`marketing-nav-${label.toLowerCase()}`}
            className={cn(
              "min-h-11 items-center px-2 text-sm underline-offset-[6px] hover:underline",
              hideOnPhone ? "hidden sm:inline-flex" : "inline-flex",
              current && "font-semibold underline decoration-2",
            )}
          >
            {label}
          </Link>
        );
      })}
    </nav>
  );
}
