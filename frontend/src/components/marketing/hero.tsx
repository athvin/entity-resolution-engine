import Link from "next/link";

import { HERO } from "@/components/marketing/content";
import { Button } from "@/components/ui/button";
import { stagger } from "@/lib/motion";
import { PORTAL_ENTRY_PATH } from "@/lib/site/routes";
import { cn } from "@/lib/utils";

/**
 * The hero.
 *
 * The visual is the product rather than an illustration: two near-duplicate
 * source records resolving into one golden record, with the evidence that
 * produced it. It is hand-rolled DOM and CSS for the same two reasons the
 * evidence waterfall is (see the frontend-data-ui skill) — pixel determinism
 * for the visual baselines, and legibility to assistive technology.
 *
 * Hierarchy comes from type scale and weight, not hue. The single inversion
 * allowed per viewport is spent on the primary CTA, which is why nothing else
 * here uses `bg-foreground`.
 *
 * ## Only the bars move
 *
 * No text on this page animates — not the headline, not the records, not the
 * golden record. Every word is legible on first paint. What animates is the
 * five evidence bars, which grow out from the centre axis in sequence, so the
 * figure's argument (the evidence accumulates, and the merge is its result) is
 * carried by the one part of the figure that is purely graphical.
 *
 * That boundary is not a stylistic preference, it is a contrast requirement,
 * and the axe gate failed before it was respected. `--muted-foreground` is set
 * to `oklch(0.5 0 0)` precisely so it clears WCAG AA at 4.5:1 — which leaves it
 * no headroom at all. An element mid-fade renders below its own opacity, so a
 * staggered fade parked this figure's muted labels under the floor and axe
 * reported `color-contrast` at `serious`.
 *
 * The rule the rest of the app now follows is in `globals.css`: muted text
 * moves, it does not fade. The bars are exempt because they have no contrast
 * to lose — they sit inside an `aria-hidden` span and animate `transform`
 * alone — which is exactly why they are the part that gets to move.
 *
 * Two further consequences worth keeping: `prefers-reduced-motion` is handled
 * by the global clamp in globals.css with no guard needed here, and Playwright
 * fast-forwards finite animations to completion for screenshots, so the
 * committed visual baseline sees the same pixels it always did.
 */

/** The five comparisons, in the order the waterfall would print them. The
 *  weights sum to the +12.8 shown as the total; changing one without the
 *  other makes the figure lie about arithmetic the product is selling. */
const EVIDENCE = [
  { field: "first name", value: "Jonathan vs Jon", weight: 1.8 },
  { field: "surname", value: "Reyes vs Reyes", weight: 4.1 },
  { field: "email local part", value: "j.reyes vs jreyes", weight: 2.6 },
  { field: "phone", value: "normalised match", weight: 5.2 },
  { field: "middle initial", value: "A. vs absent", weight: -0.9 },
] as const;

/** One comparison row, in the vocabulary of the real evidence waterfall: a
 *  signed weight, and a bar whose side of the axis carries the polarity. */
function Evidence({
  field,
  value,
  weight,
  step,
}: {
  field: string;
  value: string;
  weight: number;
  step: number;
}) {
  const supports = weight >= 0;
  // Widths are a share of the half-track, carried in a custom property so the
  // positioning stays in the class string and only the number is inline.
  const share = Math.min(Math.abs(weight) / 6, 1) * 50;
  return (
    <div className="grid grid-cols-[7rem_1fr_3rem] items-center gap-3 text-xs">
      <span className="text-muted-foreground truncate">{field}</span>
      <span className="relative h-4" aria-hidden="true">
        <span className="bg-border absolute inset-y-0 left-1/2 w-px" />
        <span
          // The bar grows *away from the centre axis*, so the direction of
          // growth is the same signal the bar's resting side carries: support
          // pushes right, objection pushes left. Hence the origin flips with
          // the sign rather than being a constant.
          className={cn(
            "bg-foreground/70 animate-bar-grow absolute inset-y-[3px] w-(--bar-w) rounded-sm",
            supports ? "left-1/2 origin-left" : "right-1/2 origin-right",
          )}
          style={
            {
              "--bar-w": `${String(Math.max(share, 2))}%`,
              ...stagger(step),
            } as React.CSSProperties
          }
        />
      </span>
      <span className="text-right font-mono tabular-nums">
        {supports ? "+" : ""}
        {weight.toFixed(1)}
      </span>
      <span className="sr-only">
        {value} — weight {weight.toFixed(1)}, {supports ? "supports the match" : "argues against"}
      </span>
    </div>
  );
}

function SourceRecord({
  source,
  name,
  email,
  phone,
}: {
  source: string;
  name: string;
  email: string;
  phone: string;
}) {
  return (
    <div className="min-w-0 rounded-md border p-3">
      <p className="text-muted-foreground mb-2 text-xs">{source}</p>
      <dl className="space-y-1 text-sm">
        <div className="truncate">
          <dt className="sr-only">Name</dt>
          <dd className="font-medium">{name}</dd>
        </div>
        <div className="text-muted-foreground truncate">
          <dt className="sr-only">Email</dt>
          <dd>{email}</dd>
        </div>
        <div className="text-muted-foreground truncate font-mono text-xs tabular-nums">
          <dt className="sr-only">Phone</dt>
          <dd>{phone}</dd>
        </div>
      </dl>
    </div>
  );
}

function ResolutionFigure() {
  return (
    <figure className="rounded-lg border p-4 sm:p-6" data-testid="marketing-hero-figure">
      <figcaption className="sr-only">
        Two source records for the same customer resolving into one golden record, with the match
        evidence that produced it.
      </figcaption>

      <div className="grid gap-3 sm:grid-cols-2">
        <SourceRecord
          source="CRM"
          name="Jonathan A. Reyes"
          email="j.reyes@acme.example"
          phone="+1 415 555 0142"
        />
        <SourceRecord
          source="Billing"
          name="Jon Reyes"
          email="jreyes@acme.example"
          phone="4155550142"
        />
      </div>

      <div className="my-4 space-y-1.5 border-y py-4">
        <p className="text-muted-foreground mb-2 text-xs">Evidence</p>
        {EVIDENCE.map((row, index) => (
          <Evidence key={row.field} {...row} step={index} />
        ))}
        <div className="flex items-baseline justify-between pt-2 text-xs">
          <span className="text-muted-foreground">total match weight</span>
          <span className="font-mono font-medium tabular-nums">+12.8</span>
        </div>
      </div>

      <div className="rounded-md border p-3">
        <p className="text-muted-foreground mb-2 text-xs">Golden record</p>
        <dl className="space-y-1 text-sm">
          <div className="truncate">
            <dt className="sr-only">Name</dt>
            <dd className="font-medium">Jonathan A. Reyes</dd>
          </div>
          <div className="text-muted-foreground truncate">
            <dt className="sr-only">Email</dt>
            <dd>j.reyes@acme.example</dd>
          </div>
          <div className="text-muted-foreground truncate font-mono text-xs tabular-nums">
            <dt className="sr-only">Phone</dt>
            <dd>+1 415 555 0142</dd>
          </div>
        </dl>
      </div>
    </figure>
  );
}

export function Hero() {
  // The headline carries a deliberate line break at a phrase boundary rather
  // than wrapping wherever the viewport happens to land.
  const [firstLine, secondLine] = HERO.headline.split("\n");
  return (
    <section
      className="mx-auto max-w-5xl px-6 pt-16 pb-20 sm:pt-24 lg:pb-28"
      data-testid="marketing-hero"
    >
      <div className="grid items-center gap-12 lg:grid-cols-[1.1fr_1fr] lg:gap-16">
        <div>
          <h1 className="text-4xl leading-[1.05] font-medium tracking-tight text-balance sm:text-5xl lg:text-6xl">
            {firstLine}
            <br />
            <span className="text-muted-foreground">{secondLine}</span>
          </h1>
          <p className="text-muted-foreground mt-6 max-w-xl text-base leading-relaxed sm:text-lg">
            {HERO.subhead}
          </p>
          <div className="mt-8 flex flex-wrap items-center gap-3">
            <Button asChild size="lg">
              <Link href={PORTAL_ENTRY_PATH} data-testid="marketing-cta-hero">
                {HERO.primaryCta}
              </Link>
            </Button>
            <Button asChild size="lg" variant="outline">
              <Link href="#how-it-works">{HERO.secondaryCta}</Link>
            </Button>
          </div>
        </div>
        <ResolutionFigure />
      </div>
    </section>
  );
}
