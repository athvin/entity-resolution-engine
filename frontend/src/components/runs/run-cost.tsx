"use client";

import { useState } from "react";
import { Calculator } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { BffRequestError } from "@/lib/api/client";
import { useBlockingEstimate } from "@/lib/query/blocking";

/** Thousands separators; the numbers here are the point of the panel. */
function count(value: number): string {
  return value.toLocaleString("en-US");
}

/**
 * How much of the cartesian product the blocking rules avoid, as a percentage
 * with enough precision to be meaningful at the top end — "99.9% avoided" and
 * "99.999% avoided" are three orders of magnitude apart in real cost.
 */
function avoided(candidate: number, total: number): string {
  if (total <= 0) return "—";
  const share = 1 - candidate / total;
  if (share <= 0) return "0%";
  const pct = share * 100;
  return `${pct >= 99.9 ? pct.toFixed(4) : pct.toFixed(1)}%`;
}

/**
 * What a run would cost, before starting it (E6a's estimator, surfaced in
 * E6c). The incumbent's answer to a long run is a three-hour cap; this is the
 * honest one — show the operator the pair count their rules generate, which
 * rule is pulling its weight, and which block is big enough to dominate.
 *
 * Fetched ON DEMAND. The estimate opens the lake and counts, so a page that
 * ran it automatically would make the Runs page expensive to open.
 */
export function RunCost({ org }: { org: string }) {
  const [asked, setAsked] = useState(false);
  const estimate = useBlockingEstimate(org, asked);

  const unavailable =
    estimate.error instanceof BffRequestError && estimate.error.status === 409
      ? estimate.error.message
      : null;

  return (
    <Card data-testid="run-cost">
      <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
        <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
          What a run would cost
        </CardTitle>
        <CardDescription>
          Counted from the model the next run will actually use, not from the config — asking the
          config would estimate a run nobody is going to perform.
        </CardDescription>
      </CardHeader>
      <CardContent className="p-4 pt-2 lg:p-6 lg:pt-2">
        {!asked ? (
          <Button
            variant="outline"
            size="sm"
            data-testid="run-cost-estimate"
            onClick={() => {
              setAsked(true);
            }}
          >
            <Calculator /> Estimate it
          </Button>
        ) : estimate.isPending ? (
          <Skeleton className="h-24 w-full" />
        ) : unavailable !== null ? (
          <p className="text-muted-foreground text-sm" data-testid="run-cost-unavailable">
            {unavailable}
          </p>
        ) : estimate.isError ? (
          <p className="text-destructive text-sm" role="alert" data-testid="run-cost-error">
            The estimate could not be computed.
          </p>
        ) : (
          <div className="flex flex-col gap-4">
            <div className="flex flex-wrap gap-6">
              <div>
                <p className="text-muted-foreground text-xs uppercase">Pairs to score</p>
                <p className="text-lg font-semibold tabular-nums" data-testid="run-cost-candidate">
                  {count(estimate.data.candidate_pairs)}
                </p>
              </div>
              <div>
                <p className="text-muted-foreground text-xs uppercase">Without blocking</p>
                <p className="text-muted-foreground text-lg tabular-nums">
                  {count(estimate.data.total_possible_pairs)}
                </p>
              </div>
              <div>
                <p className="text-muted-foreground text-xs uppercase">Avoided</p>
                <p className="text-lg font-semibold tabular-nums" data-testid="run-cost-avoided">
                  {avoided(estimate.data.candidate_pairs, estimate.data.total_possible_pairs)}
                </p>
              </div>
            </div>

            <div className="overflow-x-auto">
              <table className="w-full min-w-[32rem] text-sm">
                <thead>
                  <tr className="text-muted-foreground border-b text-left text-xs uppercase">
                    <th className="py-1.5 pr-4 font-medium">Blocking rule</th>
                    <th className="py-1.5 pr-4 font-medium">Adds</th>
                    <th className="py-1.5 font-medium">Running total</th>
                  </tr>
                </thead>
                <tbody>
                  {estimate.data.per_rule.map((rule) => (
                    <tr key={rule.match_key} className="border-b last:border-0">
                      <td className="py-2 pr-4 font-mono text-xs">{rule.blocking_rule}</td>
                      <td className="py-2 pr-4 font-medium tabular-nums">
                        {count(rule.marginal_pairs)}
                      </td>
                      <td className="text-muted-foreground py-2 tabular-nums">
                        {count(rule.cumulative_pairs)}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
              <p className="text-muted-foreground pt-2 text-xs">
                &ldquo;Adds&rdquo; is the pairs a rule generates that no earlier rule already did —
                the number that says whether it is pulling its weight. Order-dependent, in the
                model&apos;s own rule order.
              </p>
            </div>

            {estimate.data.largest_blocks.length > 0 && (
              <div data-testid="run-cost-blocks">
                <p className="text-muted-foreground pb-1 text-xs uppercase">Largest blocks</p>
                <ul className="flex flex-col gap-1 text-sm">
                  {estimate.data.largest_blocks.map((block) => (
                    <li key={block.key_values.join("|")} className="flex items-baseline gap-2">
                      <span className="font-mono text-xs">
                        {block.key_values.join(" | ") || "(blank)"}
                      </span>
                      <span className="text-muted-foreground tabular-nums">
                        {count(block.pairs)} pairs
                      </span>
                    </li>
                  ))}
                </ul>
                <p className="text-muted-foreground pt-2 text-xs">
                  One oversized block dominates a run on its own. A rule that keys on a value most
                  records share is the usual cause — restricting the run to a segment, or blocking
                  on something more selective, is the fix.
                </p>
              </div>
            )}
          </div>
        )}
      </CardContent>
    </Card>
  );
}
