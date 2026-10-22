import Link from "next/link";

import { CLOSING_CTA } from "@/components/marketing/content";
import { Button } from "@/components/ui/button";
import { PORTAL_ENTRY_PATH } from "@/lib/site/routes";

export function ClosingCta() {
  return (
    <section className="border-t" data-testid="marketing-cta">
      <div className="mx-auto max-w-5xl px-6 py-20 lg:py-28">
        <div className="max-w-xl">
          <h2 className="text-2xl font-medium tracking-tight sm:text-3xl">{CLOSING_CTA.heading}</h2>
          <p className="text-muted-foreground mt-4 leading-relaxed">{CLOSING_CTA.body}</p>
          <Button asChild size="lg" className="mt-8">
            <Link href={PORTAL_ENTRY_PATH} data-testid="marketing-cta-closing">
              {CLOSING_CTA.cta}
            </Link>
          </Button>
        </div>
      </div>
    </section>
  );
}
