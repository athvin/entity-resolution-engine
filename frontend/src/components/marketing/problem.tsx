import { PROBLEM } from "@/components/marketing/content";

export function Problem() {
  return (
    <section className="border-t" data-testid="marketing-problem">
      <div className="mx-auto max-w-5xl px-6 py-20 lg:py-28">
        <div className="max-w-2xl">
          <h2 className="text-2xl font-medium tracking-tight sm:text-3xl">{PROBLEM.heading}</h2>
          <p className="text-muted-foreground mt-4 leading-relaxed">{PROBLEM.body}</p>
        </div>
        <dl className="mt-12 grid gap-8 sm:grid-cols-3">
          {PROBLEM.costs.map((cost) => (
            <div key={cost.term}>
              <dt className="font-medium">{cost.term}</dt>
              <dd className="text-muted-foreground mt-2 text-sm leading-relaxed">{cost.detail}</dd>
            </div>
          ))}
        </dl>
      </div>
    </section>
  );
}
