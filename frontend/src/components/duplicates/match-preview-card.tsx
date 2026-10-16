"use client";

import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { WaterfallChart } from "@/components/review/waterfall-chart";
import { useMatchPreview } from "@/lib/query/duplicates";

/**
 * Score-only match preview (frontend-design §7.16): dry-run the committed model
 * over the current corpus and show what it WOULD match — writing nothing. The
 * scoring is heavy, so it runs only when the steward clicks "Preview matches".
 */
export function MatchPreviewCard({ org }: { org: string }) {
  const [run, setRun] = useState(false);
  const [open, setOpen] = useState<string | null>(null);
  const preview = useMatchPreview(org, run);
  const data = preview.data;

  return (
    <Card data-testid="match-preview">
      <CardContent className="flex flex-col gap-3 p-4">
        <div className="flex items-center gap-3">
          <div>
            <div className="font-medium">Match preview</div>
            <div className="text-muted-foreground text-sm">
              Dry-run the current model over your data. Nothing is written.
            </div>
          </div>
          <Button
            className="ml-auto"
            variant="outline"
            onClick={() => {
              setRun(true);
            }}
            disabled={run && preview.isFetching}
            data-testid="match-preview-run"
          >
            {run && preview.isFetching ? "Scoring…" : "Preview matches"}
          </Button>
        </div>

        {run && preview.isPending && <Skeleton className="h-16 w-full" />}
        {preview.isError && (
          <p className="text-destructive text-sm" role="alert" data-testid="match-preview-error">
            {preview.error instanceof Error ? preview.error.message : "preview failed"}
          </p>
        )}

        {data && (
          <>
            <div className="flex flex-wrap gap-4 text-sm" data-testid="match-preview-counts">
              <span>
                <strong>{data.would_auto_merge}</strong> would auto-merge
              </span>
              <span>
                <strong>{data.would_review}</strong> would need review
              </span>
              <span className="text-muted-foreground">{data.total} candidate pairs</span>
            </div>
            {data.sample.length > 0 && (
              <div className="flex flex-col divide-y">
                {data.sample.map((pair) => {
                  const key = `${pair.rec_a_key}|${pair.rec_b_key}`;
                  return (
                    <div key={key} className="py-2">
                      <button
                        className="flex w-full items-center gap-2 text-left text-sm"
                        onClick={() => {
                          setOpen((current) => (current === key ? null : key));
                        }}
                        data-testid="match-preview-pair"
                      >
                        <span className="font-mono text-xs">{pair.rec_a_key}</span>
                        <span className="text-muted-foreground">↔</span>
                        <span className="font-mono text-xs">{pair.rec_b_key}</span>
                        <span className="ml-auto tabular-nums">
                          {(pair.match_probability * 100).toFixed(1)}%
                        </span>
                      </button>
                      {open === key && (
                        <div className="pt-2">
                          <WaterfallChart blob={pair.evidence} />
                        </div>
                      )}
                    </div>
                  );
                })}
              </div>
            )}
          </>
        )}
      </CardContent>
    </Card>
  );
}
