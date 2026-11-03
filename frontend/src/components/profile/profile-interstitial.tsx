"use client";

import { Check, CircleAlert, Loader2 } from "lucide-react";

/** What a tier is doing right now, as the screen knows it. */
export type TierState = "idle" | "running" | "done" | "failed";

export interface TierStep {
  tier: number;
  label: string;
  /** What this tier measures, in the reader's terms — shown while it waits. */
  detail: string;
  state: TierState;
  /** Why it failed, when it did. Never a bare "failed". */
  error?: string | undefined;
}

/**
 * The tier checklist, shown while a profile runs.
 *
 * The shape is the new-source wizard's activation checklist
 * (`new-source-wizard.tsx`): one row per phase, a spinner/check/alert glyph,
 * and a retry targeted at the phase that failed rather than at the whole thing.
 * Reused because a tenant has already learned to read it once, during
 * activation, and because its per-phase retry is the behaviour that matters —
 * tier 2 walks the corpus, and re-running tier 1 to get at it would be a
 * pointless minute.
 *
 * Deliberately NOT a progress percentage. The engine reports stage events, not
 * fractions, and a bar that interpolates between them invents a number — the
 * `ThresholdEditor` lesson, where an empty array rendered as a confident "0 of
 * 0 pairs": an answer given before the question could be answered.
 */
export function ProfileInterstitial({
  steps,
  onRetry,
}: {
  steps: TierStep[];
  onRetry?: ((tier: number) => void) | undefined;
}) {
  return (
    <ol className="flex flex-col gap-2" data-testid="profile-interstitial">
      {steps.map((step) => (
        <li
          key={step.tier}
          className="flex items-start gap-2 text-xs"
          data-testid={`profile-tier-${String(step.tier)}`}
          data-state={step.state}
        >
          <span className="mt-0.5 shrink-0">
            {step.state === "running" && <Loader2 className="size-3.5 animate-spin" aria-hidden />}
            {step.state === "done" && <Check className="size-3.5" aria-hidden />}
            {step.state === "failed" && <CircleAlert className="size-3.5" aria-hidden />}
            {step.state === "idle" && (
              <span className="border-muted-foreground/40 mt-1 block size-2 rounded-full border" />
            )}
          </span>
          <span className="flex flex-col gap-0.5">
            <span className={step.state === "running" ? "font-medium" : undefined}>
              {step.label}
            </span>
            <span className="text-muted-foreground">
              {step.state === "failed"
                ? (step.error ?? "it failed, with no reason given")
                : step.detail}
            </span>
            {step.state === "failed" && onRetry && (
              <button
                type="button"
                onClick={() => {
                  onRetry(step.tier);
                }}
                className="self-start underline"
                data-testid={`profile-retry-${String(step.tier)}`}
              >
                Run tier {step.tier} again
              </button>
            )}
          </span>
          {/* The live region is on the status text, not the list, so a screen
              reader is told what changed rather than re-read the whole
              checklist on every poll. */}
          {step.state === "running" && (
            <span className="sr-only" role="status">
              {step.label} is running
            </span>
          )}
        </li>
      ))}
    </ol>
  );
}
