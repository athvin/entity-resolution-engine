import Link from "next/link";

import { Button } from "@/components/ui/button";
import { PORTAL_ENTRY_PATH } from "@/lib/site/routes";

/**
 * The marketing header. A server component with no `"use client"` and no
 * interactivity, so the landing page ships no JavaScript of its own.
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
    <header
      className="bg-background/80 sticky top-0 z-10 border-b backdrop-blur"
      data-testid="marketing-header"
    >
      <div className="mx-auto flex h-16 max-w-5xl items-center justify-between gap-4 px-6">
        <Link href="/" className="text-base font-medium tracking-tight">
          DupeZero
        </Link>
        <Button asChild size="sm">
          <Link href={PORTAL_ENTRY_PATH} data-testid="marketing-cta-header">
            Open the app
          </Link>
        </Button>
      </div>
    </header>
  );
}
