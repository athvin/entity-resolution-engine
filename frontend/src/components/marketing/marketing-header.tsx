import Link from "next/link";
import { MarketingThemeToggle } from "./theme-toggle";

import { Button } from "@/components/ui/button";
import { PORTAL_ENTRY_PATH } from "@/lib/site/routes";

/**
 * The marketing header. A server component with no `"use client"` and no
 * interactivity, so navigation works independently of the interactive demos.
 *
 * One CTA, and it is correct for both signed-in and signed-out visitors without
 * knowing which it is talking to: PORTAL_ENTRY_PATH is behind the proxy's
 * cookie gate, so the redirect happens server-side. Reading the session here
 * would make the page dynamic and defeat the static prerender; reading it in
 * the browser would cost a round trip and flash the wrong label on the one page
 * where first paint matters most.
 *
 * No mobile drawer, deliberately. The nav is a plain row at every width, which
 * keeps the mobile DOM a subset of the desktop one — the premise the axe gate
 * relies on when it sweeps desktop only (see the frontend-testing skill).
 */
export function MarketingHeader() {
  return (
    <header className="bg-background sticky top-0 z-10 border-b" data-testid="marketing-header">
      <div className="mx-auto flex h-16 max-w-7xl items-center justify-between gap-1 px-4 sm:px-6">
        <Link href="/" className="text-base font-medium tracking-tight">
          DupeZero
        </Link>
        <nav aria-label="Main navigation" className="ml-auto flex items-center gap-1 sm:gap-5">
          <Link className="hidden min-h-11 items-center px-2 text-sm sm:inline-flex" href="/">
            Home
          </Link>
          <Link className="inline-flex min-h-11 items-center px-2 text-sm" href="/docs">
            Docs
          </Link>
        </nav>
        <MarketingThemeToggle />
        <Button asChild size="sm" variant="outline" className="min-h-11">
          <Link href={PORTAL_ENTRY_PATH} data-testid="marketing-cta-header">
            Open the app
          </Link>
        </Button>
      </div>
    </header>
  );
}
