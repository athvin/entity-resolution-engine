"use client";

import { useState } from "react";
import { ChevronDown, ChevronRight, X } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { WaterfallChart } from "@/components/review/waterfall-chart";
import { probabilityLabel } from "@/components/review/decision-panel";
import { useAddAssertion, useMatchScores, type MatchScoreRow } from "@/lib/query/duplicates";
import { cn } from "@/lib/utils";

/**
 * Probability-band browse over `match_scores` (design §5.3): the audit read
 * over auto-merge's own evidence. Every row expands to its waterfall, and a
 * steward who disagrees writes the durable never-assertion right here.
 */
export function ScoreBandBrowser({ org, isSteward }: { org: string; isSteward: boolean }) {
  const [low, setLow] = useState(0.9);
  const [high, setHigh] = useState(1.0);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [lastWrite, setLastWrite] = useState<string | null>(null);
  const scores = useMatchScores(org, { low, high });
  const assertion = useAddAssertion(org);

  const rows = scores.data?.items ?? [];

  function pairKey(row: MatchScoreRow): string {
    return `${row.rec_a_key}|${row.rec_b_key}`;
  }

  function flagNotAMatch(row: MatchScoreRow) {
    assertion.mutate(
      { kind: "never", a: row.rec_a_key, b: row.rec_b_key, note: "flagged from score browse" },
      {
        onSuccess: (result) => {
          setLastWrite(
            result.status === "applied"
              ? `never-rule written for ${row.rec_a_key} · ${row.rec_b_key} — the split lands with the reconcile job`
              : `never-rule staged for ${row.rec_a_key} · ${row.rec_b_key} — queued behind the current run`,
          );
        },
      },
    );
  }

  return (
    <div className="flex flex-col gap-3" data-testid="score-band-browser">
      <Card>
        <CardContent className="flex flex-wrap items-end gap-3 p-4">
          <label className="text-sm">
            <span className="text-muted-foreground block text-xs">from probability</span>
            <input
              type="number"
              min={0}
              max={1}
              step={0.01}
              value={low}
              onChange={(event) => {
                setLow(Number(event.target.value));
              }}
              className="border-input bg-background mt-1 h-9 w-24 rounded-md border px-2 text-sm"
              aria-label="Band lower bound"
              data-testid="band-low"
            />
          </label>
          <label className="text-sm">
            <span className="text-muted-foreground block text-xs">to (exclusive)</span>
            <input
              type="number"
              min={0}
              max={1}
              step={0.01}
              value={high}
              onChange={(event) => {
                setHigh(Number(event.target.value));
              }}
              className="border-input bg-background mt-1 h-9 w-24 rounded-md border px-2 text-sm"
              aria-label="Band upper bound"
              data-testid="band-high"
            />
          </label>
          <p className="text-muted-foreground flex-1 text-xs">
            Scored pairs in the band, newest model first. Above the auto-merge threshold these
            merged silently — this is where you audit that decision.
          </p>
        </CardContent>
      </Card>

      {lastWrite && (
        <p className="text-muted-foreground text-xs" data-testid="score-write-note">
          {lastWrite}
        </p>
      )}

      {scores.isPending ? (
        <Skeleton className="h-40 w-full" />
      ) : rows.length === 0 ? (
        <Card>
          <CardContent className="text-muted-foreground p-8 text-center text-sm">
            No scored pairs between {probabilityLabel(low)} and {probabilityLabel(high)}.
          </CardContent>
        </Card>
      ) : (
        <div className="overflow-hidden rounded-xl border" data-testid="score-rows">
          {rows.map((row) => {
            const key = pairKey(row);
            const open = expanded === key;
            return (
              <div key={key} className="border-b last:border-0">
                <div
                  className={cn(
                    "flex cursor-pointer items-center gap-2 px-3 py-2.5 text-sm",
                    open && "bg-accent/50",
                  )}
                  onClick={() => {
                    setExpanded(open ? null : key);
                  }}
                  data-testid={`score-row-${row.rec_a_key}`}
                >
                  {open ? (
                    <ChevronDown className="text-muted-foreground size-4 shrink-0" />
                  ) : (
                    <ChevronRight className="text-muted-foreground size-4 shrink-0" />
                  )}
                  <span className="min-w-0 flex-1 truncate font-mono text-xs">
                    {row.rec_a_key} · {row.rec_b_key}
                  </span>
                  <span className="text-muted-foreground text-xs">{row.model_version}</span>
                  <span className="text-xs font-medium tabular-nums">
                    {probabilityLabel(row.match_probability)}
                  </span>
                </div>
                {open && (
                  <div className="bg-accent/20 flex flex-col gap-3 px-4 pt-1 pb-4">
                    <WaterfallChart blob={row.evidence} />
                    <div>
                      <Button
                        size="sm"
                        variant="outline"
                        disabled={!isSteward || assertion.isPending}
                        onClick={() => {
                          flagNotAMatch(row);
                        }}
                        title={
                          isSteward
                            ? "Write a durable never-rule for this pair"
                            : "flagging needs the steward role"
                        }
                        data-testid="flag-not-a-match"
                      >
                        <X /> Not a match
                      </Button>
                    </div>
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
