"use client";

import { useState } from "react";
import { AlertTriangle, ShieldCheck, Undo2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { useAssertions, useContradictions, useRetractAssertion } from "@/lib/query/reviews";
import { cn } from "@/lib/utils";

function ContradictionInspector({ org }: { org: string }) {
  const contradictions = useContradictions(org);
  if (contradictions.isPending || !contradictions.data) return null;
  if (contradictions.data.length === 0) {
    return (
      <Card data-testid="contradictions-clear">
        <CardContent className="text-muted-foreground flex items-center gap-2 p-3 text-sm">
          <ShieldCheck className="size-4 text-emerald-500" />
          No contradictions — every active rule can hold at once.
        </CardContent>
      </Card>
    );
  }
  return (
    <Card className="border-destructive/40" data-testid="contradictions">
      <CardHeader className="p-4 pb-1">
        <CardTitle className="text-destructive flex items-center gap-2 text-sm">
          <AlertTriangle className="size-4" />
          {contradictions.data.length} rule conflict
          {contradictions.data.length === 1 ? "" : "s"} — these cannot all hold
        </CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-2 p-4 pt-2 text-sm">
        {contradictions.data.map((finding) => (
          <div key={finding.never_assertion_id} className="rounded-md border p-2 text-xs">
            <span className="font-mono">{finding.rec_a_key}</span> must never match{" "}
            <span className="font-mono">{finding.rec_b_key}</span>, but always-assertions{" "}
            {finding.always_assertion_ids.join(", ")} connect them. Retract one side.
          </div>
        ))}
      </CardContent>
    </Card>
  );
}

export function AssertionLibrary({ org, isSteward }: { org: string; isSteward: boolean }) {
  const [includeRetracted, setIncludeRetracted] = useState(false);
  const assertions = useAssertions(org, includeRetracted);
  const retract = useRetractAssertion(org);
  const rows = (assertions.data?.pages ?? []).flatMap((page) => page.items);

  return (
    <div className="flex flex-col gap-3" data-testid="assertion-library">
      <ContradictionInspector org={org} />
      <label className="text-muted-foreground flex items-center gap-2 text-sm">
        <input
          type="checkbox"
          checked={includeRetracted}
          onChange={(event) => {
            setIncludeRetracted(event.target.checked);
          }}
          className="size-4"
        />
        show retracted
      </label>

      {assertions.isPending ? (
        <Skeleton className="h-40 w-full" />
      ) : rows.length === 0 ? (
        <Card>
          <CardContent className="text-muted-foreground p-8 text-center text-sm">
            No assertions yet — merge and unmerge decisions land here as always/never rules.
          </CardContent>
        </Card>
      ) : (
        <Card>
          <CardContent className="overflow-x-auto p-0">
            <table className="w-full min-w-[40rem] text-sm">
              <thead>
                <tr className="text-muted-foreground border-b text-left text-xs uppercase">
                  <th className="px-4 py-2.5 font-medium">Kind</th>
                  <th className="px-4 py-2.5 font-medium">Pair</th>
                  <th className="px-4 py-2.5 font-medium">By</th>
                  <th className="px-4 py-2.5 font-medium">Note</th>
                  <th className="px-4 py-2.5 text-right font-medium" />
                </tr>
              </thead>
              <tbody>
                {rows.map((assertion) => (
                  <tr
                    key={assertion.assertion_id}
                    className={cn("border-b last:border-0", !assertion.active && "opacity-50")}
                    data-testid={`assertion-${assertion.assertion_id}`}
                  >
                    <td className="px-4 py-2.5">
                      <span
                        className={cn(
                          "rounded-full px-2 py-0.5 text-xs font-medium",
                          assertion.kind === "always"
                            ? "bg-emerald-500/15 text-emerald-700"
                            : "bg-red-500/15 text-red-600",
                        )}
                      >
                        {assertion.kind}
                      </span>
                      {!assertion.active && (
                        <span className="text-muted-foreground ml-2 text-xs">retracted</span>
                      )}
                    </td>
                    <td className="px-4 py-2.5 font-mono text-xs">
                      {assertion.rec_a_key} · {assertion.rec_b_key}
                    </td>
                    <td className="text-muted-foreground max-w-40 truncate px-4 py-2.5 text-xs">
                      {assertion.created_by}
                    </td>
                    <td className="text-muted-foreground max-w-48 truncate px-4 py-2.5 text-xs">
                      {assertion.note ?? "—"}
                    </td>
                    <td className="px-4 py-2.5 text-right">
                      {isSteward && assertion.active && (
                        <Button
                          variant="ghost"
                          size="sm"
                          disabled={retract.isPending}
                          onClick={() => {
                            retract.mutate(assertion.assertion_id);
                          }}
                          data-testid={`retract-${assertion.assertion_id}`}
                        >
                          <Undo2 /> Retract
                        </Button>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </CardContent>
        </Card>
      )}
      {assertions.hasNextPage && (
        <Button
          variant="outline"
          onClick={() => void assertions.fetchNextPage()}
          disabled={assertions.isFetchingNextPage}
        >
          Load more
        </Button>
      )}
    </div>
  );
}
