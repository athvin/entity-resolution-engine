"use client";

import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { useModelEvaluation, type ConfusionMetrics } from "@/lib/query/models";

/**
 * A ratio as a percentage, or an em dash where it is null.
 *
 * **Null is not zero and must not read like it.** A null precision means the
 * model made no positive predictions on this label set; a precision of 0.00%
 * means it made them and got every one wrong. The dash carries a `title` saying
 * which, because a bare dash in a metrics row is otherwise indistinguishable
 * from a loading state or a bug.
 */
function ratio(value: number | null): { text: string; title: string } {
  if (value === null) {
    return {
      text: "—",
      title: "No prediction of this kind at this threshold, so the ratio has no denominator.",
    };
  }
  return { text: `${(value * 100).toFixed(1)}%`, title: value.toFixed(4) };
}

/** One headline figure. The threshold it was measured at is stated by the card. */
function Metric({ label, value }: { label: string; value: number | null }) {
  const { text, title } = ratio(value);
  return (
    <div className="flex flex-col gap-0.5" data-testid={`eval-${label.toLowerCase()}`}>
      <span className="text-muted-foreground text-xs tracking-wide uppercase">{label}</span>
      <span className="font-mono text-xl tabular-nums" title={title}>
        {text}
      </span>
    </div>
  );
}

/**
 * The confusion matrix as a 2x2 grid of counts.
 *
 * Laid out as predicted-vs-actual rather than listed as four numbers, because
 * the shape is the explanation: a reader looking at a low precision wants to
 * see which cell is large.
 */
function Matrix({ m }: { m: ConfusionMetrics }) {
  const cells: { key: keyof ConfusionMetrics; label: string }[] = [
    { key: "tp", label: "merged, agreed" },
    { key: "fp", label: "merged, wrong" },
    { key: "fn", label: "kept apart, missed" },
    { key: "tn", label: "kept apart, agreed" },
  ];
  return (
    <div className="bg-border grid grid-cols-2 gap-px" data-testid="eval-matrix">
      {cells.map(({ key, label }) => (
        <div key={key} className="bg-background flex flex-col gap-0.5 p-3">
          <span className="text-muted-foreground text-xs">{label}</span>
          <span className="font-mono text-lg tabular-nums" data-testid={`eval-${key}`}>
            {m[key] as number}
          </span>
        </div>
      ))}
    </div>
  );
}

/**
 * Precision and recall across the threshold range, as a hand-rolled bar pair.
 *
 * No charting library, by decision (`frontend-data-ui`): magnitude is a width
 * percentage in a CSS custom property, so the render is pixel-deterministic for
 * the visual baselines and legible to a screen reader. Recall is non-increasing
 * in the threshold, so the shape a reader is looking for is where the two
 * cross — the point past which tightening the threshold costs more matches than
 * it saves mistakes.
 */
function Curve({
  points,
  at,
}: {
  points: { threshold: number; precision: number | null; recall: number | null }[];
  at: number;
}) {
  const shown = points.filter((p) => p.threshold > 0 && p.threshold < 1);
  if (shown.length === 0) {
    return null;
  }
  return (
    <div className="flex flex-col gap-1" data-testid="eval-curve">
      {shown.map((p) => {
        const current = Math.abs(p.threshold - at) < 0.025;
        return (
          <div key={p.threshold} className="flex items-center gap-2 text-xs">
            <span
              className={`w-10 shrink-0 font-mono tabular-nums ${
                current ? "text-foreground font-semibold" : "text-muted-foreground"
              }`}
            >
              {p.threshold.toFixed(2)}
            </span>
            <div className="flex min-w-0 flex-1 flex-col gap-0.5">
              <div
                className="bg-foreground h-1.5 w-(--bar-w)"
                style={
                  { "--bar-w": `${((p.precision ?? 0) * 100).toFixed(1)}%` } as React.CSSProperties
                }
                data-testid={`eval-curve-precision-${p.threshold.toFixed(2)}`}
              />
              <div
                className="bg-foreground/70 h-1.5 w-(--bar-w)"
                style={
                  { "--bar-w": `${((p.recall ?? 0) * 100).toFixed(1)}%` } as React.CSSProperties
                }
                data-testid={`eval-curve-recall-${p.threshold.toFixed(2)}`}
              />
            </div>
          </div>
        );
      })}
      <p className="text-muted-foreground mt-1 text-xs">
        Upper bar precision, lower bar recall. The bold threshold is the one in force.
      </p>
    </div>
  );
}

/**
 * How a model version scored against the tenant's own verdicts.
 *
 * The engine has always been able to compute this — `dedupe-core::accuracy` —
 * and until labels existed there was nothing to measure against. The figures
 * are deliberately reported beside their exclusions: an accuracy measured over
 * four labels has to be distinguishable from one measured over four hundred,
 * and only labels carrying *this* version's own score can be used at all.
 */
export function ModelEvaluation({
  org,
  name,
  version,
}: {
  org: string;
  name: string;
  version: number;
}) {
  const evaluation = useModelEvaluation(org, name, version);
  const data = evaluation.data;

  return (
    <Card data-testid="model-evaluation">
      <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
        <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
          {name} v{version} — measured accuracy
        </CardTitle>
        <CardDescription>
          {data === undefined ? (
            "Scored against the verdicts your stewards recorded while this version was the one producing scores."
          ) : (
            <>
              Against {data.counts.evaluable} labelled{" "}
              {data.counts.evaluable === 1 ? "pair" : "pairs"} at the auto-merge threshold{" "}
              <span className="font-mono tabular-nums">{data.threshold.toFixed(2)}</span>. Only
              labels carrying this version&apos;s own score can be measured, which is what makes
              this an evaluation of <span className="font-mono">{data.model_version_label}</span>{" "}
              rather than of whatever is active now.
            </>
          )}
        </CardDescription>
      </CardHeader>
      <CardContent className="p-4 pt-2 lg:p-6 lg:pt-3">
        {evaluation.isPending && <Skeleton className="h-28" data-testid="eval-loading" />}

        {!evaluation.isPending && data !== undefined && data.counts.evaluable === 0 && (
          <div className="flex flex-col gap-1 text-sm" data-testid="eval-empty">
            <p className="text-muted-foreground">
              No label can be scored against this version yet.
            </p>
            <p className="text-muted-foreground text-xs">
              {data.counts.labels_for_version} recorded under it
              {data.counts.no_score > 0 && (
                <>
                  , {data.counts.no_score} with no score shown to the labeller (resolving a review
                  records a verdict without a probability in front of it)
                </>
              )}
              {data.counts.not_a_verdict > 0 && <>, {data.counts.not_a_verdict} not a verdict</>}.
              Label pairs from the review queue while this version is active to measure it.
            </p>
          </div>
        )}

        {!evaluation.isPending && data !== undefined && data.counts.evaluable > 0 && (
          <div className="flex flex-col gap-4">
            <div className="flex flex-wrap gap-x-8 gap-y-3">
              <Metric label="Precision" value={data.overall.precision} />
              <Metric label="Recall" value={data.overall.recall} />
              <Metric label="F1" value={data.overall.f1} />
              <Metric label="Accuracy" value={data.overall.accuracy} />
            </div>

            <Matrix m={data.overall} />

            {Object.keys(data.by_origin).length > 1 && (
              <div className="flex flex-col gap-1" data-testid="eval-by-origin">
                <span className="text-muted-foreground text-xs tracking-wide uppercase">
                  By label source
                </span>
                <table className="w-full text-sm">
                  <thead>
                    <tr className="text-muted-foreground text-xs">
                      <th className="py-1 text-left font-medium">source</th>
                      <th className="py-1 text-right font-medium">labels</th>
                      <th className="py-1 text-right font-medium">precision</th>
                      <th className="py-1 text-right font-medium">recall</th>
                    </tr>
                  </thead>
                  <tbody>
                    {Object.entries(data.by_origin).map(([origin, m]) => (
                      <tr key={origin} className="border-border border-t">
                        <td className="py-1">{origin}</td>
                        <td className="py-1 text-right font-mono tabular-nums">
                          {m.tp + m.fp + m.fn + m.tn}
                        </td>
                        <td className="py-1 text-right font-mono tabular-nums">
                          {ratio(m.precision).text}
                        </td>
                        <td className="py-1 text-right font-mono tabular-nums">
                          {ratio(m.recall).text}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
                <p className="text-muted-foreground text-xs">
                  Kept apart because a number measured on a model&apos;s own suggestions is not the
                  same claim as one measured on a human&apos;s.
                </p>
              </div>
            )}

            <Curve points={data.curve} at={data.threshold} />
          </div>
        )}

        {evaluation.isError && (
          <p className="text-destructive text-sm" role="alert" data-testid="eval-error">
            could not load the evaluation
          </p>
        )}
      </CardContent>
    </Card>
  );
}
