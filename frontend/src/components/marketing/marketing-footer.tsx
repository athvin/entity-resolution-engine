import Link from "next/link";

import { FOOTER } from "@/components/marketing/content";
import { FOOTER_COLUMNS } from "@/components/marketing/site-links";
import { PORTAL_ENTRY_PATH } from "@/lib/site/routes";

export function MarketingFooter() {
  return (
    <footer className="border-t" data-testid="marketing-footer">
      <div className="text-muted-foreground mx-auto grid max-w-7xl gap-10 px-4 py-12 text-sm sm:px-6 md:grid-cols-[1.4fr_repeat(3,1fr)]">
        <div>
          <p>
            <span className="text-foreground font-medium">{FOOTER.note}</span> — {FOOTER.tagline}
          </p>
          <Link
            href={PORTAL_ENTRY_PATH}
            className="hover:text-foreground mt-3 inline-flex min-h-11 items-center underline underline-offset-4"
          >
            Sign in
          </Link>
        </div>
        {FOOTER_COLUMNS.map((col) => (
          <nav key={col.heading} aria-label={`${col.heading} links`}>
            <h2 className="text-foreground mb-2 text-sm font-semibold">{col.heading}</h2>
            <ul>
              {col.links.map((l) => (
                <li key={l.href}>
                  <Link
                    href={l.href}
                    className="hover:text-foreground inline-flex min-h-11 items-center underline-offset-4 hover:underline"
                  >
                    {l.label}
                  </Link>
                </li>
              ))}
            </ul>
          </nav>
        ))}
      </div>
    </footer>
  );
}
