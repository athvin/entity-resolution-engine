import Link from "next/link";

import { FOOTER } from "@/components/marketing/content";
import { PORTAL_ENTRY_PATH } from "@/lib/site/routes";

export function MarketingFooter() {
  return (
    <footer className="border-t" data-testid="marketing-footer">
      <div className="text-muted-foreground mx-auto flex max-w-5xl flex-col gap-2 px-6 py-10 text-sm sm:flex-row sm:items-center sm:justify-between">
        <p>
          <span className="text-foreground font-medium">{FOOTER.note}</span> — {FOOTER.tagline}
        </p>
        <Link href={PORTAL_ENTRY_PATH} className="hover:text-foreground underline-offset-4">
          Sign in
        </Link>
      </div>
    </footer>
  );
}
