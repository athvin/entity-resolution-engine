import type { Metadata } from "next";
import Link from "next/link";

import "@/components/marketing/story.css";
import { MarketingFooter } from "@/components/marketing/marketing-footer";
import { MarketingHeader } from "@/components/marketing/marketing-header";
import { Page, PageIntro } from "@/components/marketing/pages/page-kit";

/**
 * The 404 page, inside the root layout. Next returns status 404 and adds
 * `noindex` itself, so this is never a soft 404.
 *
 * It shows for every `notFound()` (an unknown blog post, guide or solution) and
 * for unmatched multi-segment paths. A single unknown segment such as `/acme`
 * is not a 404: it may be a customer workspace, so the proxy sends a signed-out
 * visitor to login, and `/login` is disallowed in robots.txt.
 */
export const metadata: Metadata = {
  title: "Page not found",
  description: "This page does not exist. Find DupeZero's product, pricing, docs and guides here.",
};

const KEY_PAGES = [
  { href: "/", label: "Home" },
  { href: "/product", label: "Product overview" },
  { href: "/solutions", label: "Solutions by role and industry" },
  { href: "/pricing", label: "Pricing" },
  { href: "/docs", label: "Developer docs" },
  { href: "/resources", label: "Guides" },
  { href: "/blog", label: "Blog" },
  { href: "/contact", label: "Contact" },
] as const;

export default function NotFound() {
  return (
    <div className="marketing-site dark bg-background text-foreground flex min-h-dvh flex-col">
      <MarketingHeader />
      <main className="flex-1">
        <Page testId="page-not-found">
          <PageIntro
            eyebrow="404"
            title="This page does not exist."
            lead="The link may be out of date, or the address mistyped. These pages are a good place to start."
          />
          <ul className="page-bullets" data-testid="not-found-links">
            {KEY_PAGES.map((p) => (
              <li key={p.href}>
                <Link href={p.href} className="story-text-link">
                  {p.label}
                </Link>
              </li>
            ))}
          </ul>
        </Page>
      </main>
      <MarketingFooter />
    </div>
  );
}
