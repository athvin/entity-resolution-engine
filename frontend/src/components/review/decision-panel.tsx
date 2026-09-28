"use client";

import { Check, SkipForward, X } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import type { Resolution, ReviewRow } from "@/lib/query/reviews";
import { WaterfallChart } from "./waterfall-chart";

export function probabilityLabel(probability: number | null): string {
  if (probability === null) return "no score";
  return `${(probability * 100).toFixed(1)}%`;
}

export const REASON_COPY: Record<string, string> = {
  gray_band: "scored between your review threshold and auto-merge — the engine is asking you",
  never_unsatisfiable: "a never-assertion cannot be honored without your help",
  coherence: "the entity looks internally inconsistent",
};

interface DecisionPanelProps {
  review: ReviewRow;
  disabled: boolean;
  onResolve: (resolution: Resolution) => void;
  showKeyHints: boolean;
}

/** The right-hand decision panel (design §5.3): the pair, why it's here, and
 * the evidence waterfall — with the three verdicts. */
export function DecisionPanel({ review, disabled, onResolve, showKeyHints }: DecisionPanelProps) {
  return (
    <Card className="flex h-full flex-col" data-testid="decision-panel">
      <CardHeader className="p-4 pb-2 lg:p-6 lg:pb-2">
        <CardTitle className="flex flex-wrap items-baseline gap-x-3 gap-y-1 text-base">
          <span className="font-mono text-sm">{review.rec_a_key ?? "?"}</span>
          <span className="text-muted-foreground text-xs">vs</span>
          <span className="font-mono text-sm">{review.rec_b_key ?? "?"}</span>
          <span
            className="bg-secondary text-secondary-foreground ml-auto rounded-full px-2 py-0.5 font-sans text-xs"
            data-testid="review-probability"
          >
            {probabilityLabel(review.match_probability)}
          </span>
        </CardTitle>
        <p className="text-muted-foreground text-xs">
          {REASON_COPY[review.reason] ?? review.reason}
        </p>
      </CardHeader>
      <CardContent className="flex min-h-0 flex-1 flex-col gap-4 p-4 pt-2 lg:p-6 lg:pt-2">
        <div className="min-h-0 flex-1 overflow-y-auto">
          <WaterfallChart blob={review.waterfall} />
        </div>
        <div className="grid grid-cols-3 gap-2">
          <Button
            variant="default"
            disabled={disabled}
            onClick={() => {
              onResolve("match");
            }}
            data-testid="resolve-match"
          >
            <Check />
            Match
            {showKeyHints && (
              <kbd className="bg-primary-foreground/20 ml-1 rounded px-1 text-[10px]">m</kbd>
            )}
          </Button>
          <Button
            variant="outline"
            disabled={disabled}
            onClick={() => {
              onResolve("no_match");
            }}
            data-testid="resolve-no-match"
          >
            <X />
            Not a match
            {showKeyHints && <kbd className="bg-muted ml-1 rounded px-1 text-[10px]">n</kbd>}
          </Button>
          <Button
            variant="ghost"
            disabled={disabled}
            onClick={() => {
              onResolve("dismiss");
            }}
            data-testid="resolve-dismiss"
          >
            <SkipForward />
            Dismiss{showKeyHints && <kbd className="bg-muted ml-1 rounded px-1 text-[10px]">x</kbd>}
          </Button>
        </div>
      </CardContent>
    </Card>
  );
}
