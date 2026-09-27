"use client";

import { describeRow, parseEvidence } from "@/lib/domain/evidence";
import { cn } from "@/lib/utils";

/**
 * The evidence waterfall (design §5.3): every comparison's log2 match weight as
 * a horizontal bar from a centre axis — right supports the match, left argues
 * against. Deliberately hand-rolled DOM/CSS (no canvas): pixel-deterministic
 * for visual baselines and legible to assistive tech.
 */
export function WaterfallChart({ blob }: { blob: Record<string, unknown> | null | undefined }) {
  const evidence = parseEvidence(blob);
  if (evidence.rows.length === 0) {
    return <p className="text-muted-foreground text-sm">No evidence recorded for this pair.</p>;
  }
  const maxMagnitude = Math.max(...evidence.rows.map((row) => Math.abs(row.weight)), 0.1);

  return (
    <div className="flex flex-col gap-1.5" data-testid="waterfall-chart">
      {evidence.rows.map((row) => {
        const share = Math.abs(row.weight) / maxMagnitude;
        return (
          <div key={row.column} className="grid grid-cols-[1fr_auto] items-center gap-3 text-sm">
            <div className="relative h-6">
              {/* centre axis */}
              <span className="bg-border absolute inset-y-0 left-1/2 w-px" />
              <span
                className={cn(
                  "absolute inset-y-1 rounded-sm",
                  row.supports ? "left-1/2 bg-emerald-500/70" : "right-1/2 bg-red-500/70",
                )}
                style={{ width: `${String(Math.max(share * 50, 1.5))}%` }}
                data-testid={`waterfall-bar-${row.column}`}
              />
              <span className="text-muted-foreground absolute inset-y-0 left-0 flex items-center text-xs">
                {describeRow(row)}
              </span>
            </div>
            <span
              className={cn(
                "w-12 text-right font-mono text-xs tabular-nums",
                row.supports
                  ? "text-emerald-600 dark:text-emerald-400"
                  : "text-red-600 dark:text-red-400",
              )}
            >
              {row.weight >= 0 ? "+" : ""}
              {row.weight.toFixed(1)}
            </span>
          </div>
        );
      })}
      {evidence.totalWeight !== null && (
        <div className="text-muted-foreground mt-1 border-t pt-1.5 text-right text-xs">
          total match weight{" "}
          <span className="text-foreground font-mono font-medium">
            {evidence.totalWeight >= 0 ? "+" : ""}
            {evidence.totalWeight.toFixed(1)}
          </span>
        </div>
      )}
    </div>
  );
}
