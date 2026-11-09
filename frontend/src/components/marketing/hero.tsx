import Link from "next/link";
import { ArrowRight } from "lucide-react";
import { HERO } from "@/components/marketing/content";
import { FlowDemo } from "@/components/marketing/demo/flow-demo";
import { Button } from "@/components/ui/button";
import { PORTAL_ENTRY_PATH } from "@/lib/site/routes";

export function Hero() {
  return (
    <section
      className="mx-auto max-w-7xl px-4 pt-14 pb-20 sm:px-6 sm:pt-20 lg:pb-28"
      data-testid="marketing-hero"
    >
      <div className="space-y-12">
        <div className="mx-auto max-w-3xl text-center">
          <p className="text-muted-foreground mb-6 text-xs font-medium tracking-widest uppercase">
            Resolve · Review · Reconcile
          </p>
          <h1 className="text-4xl leading-[1.08] font-medium tracking-tight text-balance sm:text-6xl lg:text-7xl">
            {HERO.headline.split("\n").map((line, index) => (
              <span className={`block ${index === 1 ? "text-muted-foreground" : ""}`} key={line}>
                {line}
              </span>
            ))}
          </h1>
          <p className="text-muted-foreground mx-auto mt-6 max-w-2xl text-base leading-relaxed">
            {HERO.subhead}
          </p>
          <div className="mt-8 flex flex-wrap items-center justify-center gap-3">
            <Button asChild size="lg">
              <Link href={PORTAL_ENTRY_PATH} data-testid="marketing-cta-hero">
                {HERO.primaryCta}
                <ArrowRight aria-hidden="true" />
              </Link>
            </Button>
            <Button asChild variant="ghost" size="lg">
              <Link href="#how-it-works">{HERO.secondaryCta}</Link>
            </Button>
          </div>
          <p className="text-muted-foreground mt-6 text-sm">
            We build the connections your stack needs.
          </p>
        </div>
        <FlowDemo />
      </div>
    </section>
  );
}
