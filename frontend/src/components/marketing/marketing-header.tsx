import Image from "next/image";
import Link from "next/link";
import { MarketingNav } from "./marketing-nav";

import { Button } from "@/components/ui/button";
import { PORTAL_ENTRY_PATH } from "@/lib/site/routes";
import { DEMO_PATH } from "./site-links";

/**
 * The marketing header. A server component with no `"use client"`; only the
 * nav (to mark the current page and open its menus) is a client island, so
 * navigation works independently of the interactive demos. There is no theme
 * control: the marketing site is always dark (see `app/providers.tsx`).
 *
 * Two buttons: Book a demo, the site's primary action, and Login. Login is
 * correct for both signed-in and signed-out visitors without knowing which it
 * is talking to: PORTAL_ENTRY_PATH is behind the proxy's
 * cookie gate, so the redirect happens server-side. Reading the session here
 * would make the page dynamic and defeat the static prerender; reading it in
 * the browser would cost a round trip and flash the wrong label on the one page
 * where first paint matters most.
 *
 * Below `sm` the row has no room for both, so Login moves into the Menu panel
 * (`marketing-nav.tsx`) and the footer's Sign in link.
 */
export function MarketingHeader() {
  return (
    <header className="bg-background sticky top-0 z-10 border-b" data-testid="marketing-header">
      <div className="mx-auto flex h-16 max-w-7xl items-center justify-between gap-1 px-4 sm:px-6">
        {/* Every theme and width variant is in the markup and CSS shows one, so
            the right logo is in the static HTML before the theme script runs.
            Below `sm` the mark alone, because the wordmark, the Menu button
            and the demo button do not fit a 320px row. SVG `src` is served unoptimized automatically. */}
        <Link href="/" className="inline-flex min-h-11 shrink-0 items-center">
          <span className="inline-flex sm:hidden">
            <Image
              src="/brand/dupezero-icon-on-light.svg"
              alt="DupeZero"
              width={36}
              height={36}
              priority
              className="size-9 dark:hidden"
            />
            <Image
              src="/brand/dupezero-icon-on-dark.svg"
              alt="DupeZero"
              width={36}
              height={36}
              priority
              className="hidden size-9 dark:block"
            />
          </span>
          <span className="hidden sm:inline-flex">
            <Image
              src="/brand/dupezero-logo-on-light.svg"
              alt="DupeZero"
              width={158}
              height={40}
              priority
              className="h-10 w-auto dark:hidden"
            />
            <Image
              src="/brand/dupezero-logo-on-dark.svg"
              alt="DupeZero"
              width={158}
              height={40}
              priority
              className="hidden h-10 w-auto dark:block"
            />
          </span>
        </Link>
        <MarketingNav />
        <Button asChild size="sm" variant="outline" className="min-h-11 shrink-0 max-sm:hidden">
          <Link href={PORTAL_ENTRY_PATH} data-testid="marketing-cta-header">
            Login
          </Link>
        </Button>
        <Button asChild size="sm" className="min-h-11 shrink-0">
          <Link href={DEMO_PATH} data-testid="marketing-cta-demo">
            Book a demo
          </Link>
        </Button>
      </div>
    </header>
  );
}
