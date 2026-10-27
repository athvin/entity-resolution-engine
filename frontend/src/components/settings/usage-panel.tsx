"use client";

import { Gauge } from "lucide-react";

import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import type { UsageRow } from "@/lib/query/usage";
import { usageLabel } from "@/lib/query/usage";

export interface UsagePanelProps {
  rows: UsageRow[] | undefined;
  days: number;
  pending: boolean;
  failed: boolean;
}

/**
 * What this workspace has consumed (O12b).
 *
 * **Metering, not billing** — there is no price here, and the copy says so
 * rather than leaving a reader to wonder what it costs. The pricing story is
 * per-run on cheap compute, and a figure nobody can reconcile against an
 * invoice would be worse than none.
 *
 * It sits on the AI settings page because that is where the lever is: someone
 * reading a surprising token count is one scroll from the switch that turns it
 * off or the field that points it at their own key.
 */
export function UsagePanel({ rows, days, pending, failed }: UsagePanelProps) {
  const total = (rows ?? []).reduce((sum, row) => sum + row.units, 0);
  return (
    <Card data-testid="usage-panel">
      <CardHeader>
        <CardTitle className="flex items-center gap-2 text-base">
          <Gauge className="size-4" aria-hidden />
          Usage, last {days} days
        </CardTitle>
        <CardDescription>
          What this workspace has consumed. A count, not a bill — nothing here is priced.
        </CardDescription>
      </CardHeader>
      <CardContent>
        {pending ? (
          <div className="flex flex-col gap-2">
            <Skeleton className="h-4 w-40" />
            <Skeleton className="h-4 w-64" />
          </div>
        ) : failed ? (
          <p className="text-muted-foreground text-sm" data-testid="usage-failed">
            could not load usage for this workspace
          </p>
        ) : total === 0 ? (
          <p className="text-muted-foreground text-sm" data-testid="usage-empty">
            Nothing metered yet — AI features record their usage here as they are used.
          </p>
        ) : (
          <table className="w-full text-sm" data-testid="usage-table">
            <thead>
              <tr className="text-muted-foreground text-left text-xs">
                <th className="font-medium">What</th>
                <th className="font-medium tabular-nums">Units</th>
                <th className="font-medium tabular-nums">Calls</th>
              </tr>
            </thead>
            <tbody>
              {(rows ?? []).map((row) => (
                <tr key={row.kind} data-testid={`usage-row-${row.kind}`}>
                  <td className="py-1">{usageLabel(row.kind)}</td>
                  <td className="py-1 tabular-nums">
                    {row.units.toLocaleString()} {row.unit}
                  </td>
                  <td className="text-muted-foreground py-1 tabular-nums">{row.events}</td>
                </tr>
              ))}
              <tr className="border-border border-t font-medium">
                <td className="py-1">Total</td>
                <td className="py-1 tabular-nums" data-testid="usage-total">
                  {total.toLocaleString()}
                </td>
                <td />
              </tr>
            </tbody>
          </table>
        )}
      </CardContent>
    </Card>
  );
}
