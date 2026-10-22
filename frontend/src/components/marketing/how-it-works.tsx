import { HOW_IT_WORKS } from "@/components/marketing/content";

/**
 * The three stages.
 *
 * Numbered markers are used here because the content genuinely is a sequence —
 * resolve, then review, then reconcile, in that order, with nothing applied
 * until all three have run. The design-system skill's warning about `01 / 02 /
 * 03` as a generic default applies to content that is merely a list; this is
 * not one.
 */
export function HowItWorks() {
  return (
    <section id="how-it-works" className="border-t" data-testid="marketing-how-it-works">
      <div className="mx-auto max-w-5xl px-6 py-20 lg:py-28">
        <div className="max-w-2xl">
          <h2 className="text-2xl font-medium tracking-tight sm:text-3xl">
            {HOW_IT_WORKS.heading}
          </h2>
          <p className="text-muted-foreground mt-4 leading-relaxed">{HOW_IT_WORKS.intro}</p>
        </div>
        <ol className="mt-12 grid gap-10 sm:grid-cols-3">
          {HOW_IT_WORKS.stages.map((stage, index) => (
            <li key={stage.name} className="border-t pt-5">
              <p className="text-muted-foreground font-mono text-xs tabular-nums">
                {String(index + 1).padStart(2, "0")}
              </p>
              <h3 className="mt-3 font-medium">{stage.name}</h3>
              <p className="mt-2 text-sm leading-relaxed">{stage.summary}</p>
              <p className="text-muted-foreground mt-3 text-sm leading-relaxed">{stage.detail}</p>
            </li>
          ))}
        </ol>
      </div>
    </section>
  );
}
