"use client";

import { Check, Sparkles, SkipForward, X } from "lucide-react";

import { Button } from "@/components/ui/button";
import type { ReviewRecommendation } from "@/lib/query/adjudicate";
import { cn } from "@/lib/utils";

/**
 * The glyph per verdict.
 *
 * Shape, not hue — the palette is unavailable (`docs/frontend-design.md` §3.3)
 * and these three states are exactly the kind a product would otherwise colour
 * green/red/grey. The same three glyphs the decision buttons use, so the
 * model's answer reads as "this button" rather than as a separate vocabulary.
 */
const GLYPHS = {
  match: Check,
  no_match: X,
  skip: SkipForward,
} as const;

const WORDS: Record<string, string> = {
  match: "same entity",
  no_match: "different entities",
  skip: "not settled by the evidence",
};

/** A verdict the UI does not know is rendered as itself rather than hidden. */
function glyphFor(verdict: string) {
  return verdict in GLYPHS ? GLYPHS[verdict as keyof typeof GLYPHS] : Sparkles;
}

export interface RecommendationBadgeProps {
  recommendation: ReviewRecommendation | undefined;
  /** False for a viewer: reading an opinion is free, asking for one spends tokens. */
  canRequest: boolean;
  busy: boolean;
  failure: string | null;
  onRequest: () => void;
}

/**
 * The AI recommendation on one review pair (V6).
 *
 * **Advisory, and the copy has to keep saying so.** This never resolves
 * anything: the steward still presses one of the three buttons below it, and
 * that resolve is the audited act. A badge that reads like a decision already
 * made is the failure mode here — hence "suggests", the explicit confidence,
 * and the model's name on every answer.
 */
export function RecommendationBadge({
  recommendation,
  canRequest,
  busy,
  failure,
  onRequest,
}: RecommendationBadgeProps) {
  if (recommendation === undefined) {
    return (
      <div className="flex flex-wrap items-center gap-2" data-testid="recommendation-empty">
        {canRequest ? (
          <Button
            variant="outline"
            size="sm"
            disabled={busy}
            onClick={onRequest}
            data-testid="recommendation-request"
          >
            <Sparkles className="size-3.5" aria-hidden />
            {busy ? "Asking…" : "Ask AI"}
          </Button>
        ) : (
          <span className="text-muted-foreground text-xs" data-testid="recommendation-none">
            No AI recommendation for this pair.
          </span>
        )}
        {failure !== null ? (
          <span className="text-xs font-medium" role="alert" data-testid="recommendation-error">
            {failure}
          </span>
        ) : null}
      </div>
    );
  }

  const Glyph = glyphFor(recommendation.verdict);
  return (
    <div
      className="border-border flex flex-col gap-1.5 rounded-md border p-2.5"
      data-testid="recommendation"
    >
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-sm">
        <Sparkles className="size-3.5 shrink-0" aria-hidden />
        <span className="text-muted-foreground">AI suggests</span>
        <span
          className={cn("flex items-center gap-1 font-medium")}
          data-testid="recommendation-verdict"
        >
          <Glyph className="size-3.5 shrink-0" aria-hidden />
          {WORDS[recommendation.verdict] ?? recommendation.verdict}
        </span>
        <span
          className="bg-secondary text-secondary-foreground ml-auto rounded-full px-2 py-0.5 text-xs tabular-nums"
          data-testid="recommendation-confidence"
        >
          {(recommendation.confidence * 100).toFixed(0)}% sure
        </span>
      </div>
      <p className="text-muted-foreground text-xs" data-testid="recommendation-reasoning">
        {recommendation.reasoning}
      </p>
      <div className="text-muted-foreground flex flex-wrap items-center gap-x-2 text-[11px]">
        {/* The model is named on every answer: an accuracy claim that does not
            say whose it is cannot be checked later. */}
        <span data-testid="recommendation-model">{recommendation.model}</span>
        <span aria-hidden>·</span>
        <span>a suggestion — your resolve below is what counts</span>
        {canRequest ? (
          <button
            type="button"
            className="hover:text-foreground ml-auto underline underline-offset-2"
            disabled={busy}
            onClick={onRequest}
            data-testid="recommendation-redo"
          >
            {busy ? "Asking…" : "Ask again"}
          </button>
        ) : null}
      </div>
      {failure !== null ? (
        <span className="text-xs font-medium" role="alert" data-testid="recommendation-error">
          {failure}
        </span>
      ) : null}
    </div>
  );
}
