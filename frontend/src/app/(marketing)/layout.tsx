import type { Metadata } from "next";

import { MarketingFooter } from "@/components/marketing/marketing-footer";
import { MarketingHeader } from "@/components/marketing/marketing-header";
import { CANONICAL_ORIGIN } from "@/lib/site/routes";

/**
 * The public marketing surface.
 *
 * A server component with no `"use client"`, and it must stay that way — the
 * whole point of this route group is a page that prerenders to static HTML.
 * It renders no app shell: `AppShell` is mounted only by
 * `(tenant)/[org]/layout.tsx` and `AdminShell` only by `(admin)/admin`, so a
 * sibling route group inherits neither.
 *
 * `metadataBase` is mandatory the moment a page declares a canonical tag or an
 * OG image — a relative URL in a metadata field is a build error without it.
 * It comes from a hardcoded constant rather than an environment variable; the
 * reasoning is in `src/lib/site/routes.ts`, and the short version is that a
 * canonical origin is one value for the whole internet by definition, while
 * `next build` runs with no runtime environment and one image serves many
 * hostnames.
 */
export const metadata: Metadata = {
  metadataBase: new URL(CANONICAL_ORIGIN),
  openGraph: {
    type: "website",
    siteName: "DupeZero",
    locale: "en_US",
    url: "/",
  },
  twitter: { card: "summary_large_image" },
};

export default function MarketingLayout({ children }: { children: React.ReactNode }) {
  return (
    <div className="flex min-h-dvh flex-col">
      <MarketingHeader />
      <main className="flex-1">{children}</main>
      <MarketingFooter />
    </div>
  );
}
