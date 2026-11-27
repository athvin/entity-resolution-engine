import { PROOF } from "@/components/marketing/content";

export function Proof() {
  return (
    <section className="border-t" data-testid="marketing-proof">
      <div className="mx-auto max-w-5xl px-6 py-20 lg:py-28">
        <h2 className="max-w-2xl text-2xl font-medium tracking-tight sm:text-3xl">
          {PROOF.heading}
        </h2>
        <dl className="mt-12 grid gap-x-12 gap-y-10 sm:grid-cols-2">
          {PROOF.points.map((point) => (
            <div key={point.term} className="border-t pt-5">
              <dt className="font-medium">{point.term}</dt>
              <dd className="text-muted-foreground mt-2 text-sm leading-relaxed">{point.detail}</dd>
            </div>
          ))}
        </dl>
      </div>
    </section>
  );
}
