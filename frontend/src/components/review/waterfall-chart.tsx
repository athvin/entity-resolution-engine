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
          <div
            key={row.column}
            className="grid grid-cols-[1fr_auto] items-end gap-3 text-sm sm:items-center"
          >
            {/* Below sm the label sits above its bar; wider, it overlays the bar's left half. */}
            <div className="relative min-w-0">
              <span className="text-muted-foreground mb-0.5 block text-xs sm:absolute sm:inset-y-0 sm:left-0 sm:z-10 sm:mb-0 sm:flex sm:items-center">
                {describeRow(row)}
              </span>
              <div className="relative h-6">
                {/* centre axis */}
                <span className="bg-border absolute inset-y-0 left-1/2 w-px" />
                <span
                  className={cn(
                    "absolute inset-y-1 w-(--bar-w) rounded-sm",
                    row.supports ? "left-1/2 bg-emerald-500/70" : "right-1/2 bg-red-500/70",
                  )}
                  style={
                    { "--bar-w": `${String(Math.max(share * 50, 1.5))}%` } as React.CSSProperties
                  }
                  data-testid={`waterfall-bar-${row.column}`}
                />
              </div>
            </div>
            <span
              className={cn(
                "w-12 text-right font-mono text-xs tabular-nums",
                row.supports
                  ? "text-emerald-700 dark:text-emerald-400"
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
