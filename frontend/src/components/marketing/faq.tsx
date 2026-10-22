import { FAQ } from "@/components/marketing/content";

/**
 * Native `<details>` / `<summary>`, not a Radix accordion.
 *
 * It is keyboard-operable and screen-reader-correct with no work, it works
 * before hydration, it is deterministic for the visual baselines, and it adds
 * nothing to the lockfile — so no new entry has to clear the `pnpm audit` and
 * licence gates. The design-system skill's rule is to check for a native
 * element before reaching for a primitive; this is the case it was written for.
 *
 * The trade accepted: no height transition on open. A marker rotation is the
 * whole affordance, and it is a CSS transition so the `prefers-reduced-motion`
 * clamp in globals.css covers it.
 */
export function Faq() {
  return (
    <section className="border-t" data-testid="marketing-faq">
      <div className="mx-auto max-w-3xl px-6 py-20 lg:py-28">
        <h2 className="text-2xl font-medium tracking-tight sm:text-3xl">{FAQ.heading}</h2>
        <div className="mt-10 divide-y border-y">
          {FAQ.items.map((item) => (
            <details key={item.q} className="group py-4">
              <summary className="flex cursor-pointer items-center justify-between gap-4 font-medium [&::-webkit-details-marker]:hidden">
                {item.q}
                <span
                  aria-hidden="true"
                  className="text-muted-foreground shrink-0 transition-transform duration-150 group-open:rotate-45"
                >
                  +
                </span>
              </summary>
              <p className="text-muted-foreground mt-3 text-sm leading-relaxed">{item.a}</p>
            </details>
          ))}
        </div>
      </div>
    </section>
  );
}
