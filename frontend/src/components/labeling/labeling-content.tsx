"use client";

import { useCallback, useEffect, useState } from "react";

import { FieldCompareGrid } from "@/components/review/field-compare-grid";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { BffRequestError } from "@/lib/api/client";
import { effectiveRole, useSession } from "@/lib/query/hooks";
import {
  canRecordLabel,
  labelPayloadFor,
  nextUnskipped,
  pairKey,
  useLabelingQueue,
  useRecordLabel,
  type Verdict,
} from "@/lib/query/labeling";

/**
 * Teaching the matcher, one pair at a time.
 *
 * E10b built the labels table and the uncertainty-sampled queue and shipped
 * **no surface at all** — so a tenant could not record a verdict, which meant
 * `m_from_labels` had nothing to train on and per-version evaluation had
 * nothing to measure. This is that surface. It is the input the evaluation
 * panel on the models page reads.
 *
 * Two decisions worth keeping:
 *
 * - **The queue's `model_version` and `score_at_label` are sent back with the
 *   verdict.** Without them the label is still good training signal but is
 *   invisible to per-version evaluation, which can only measure labels carrying
 *   the score that version actually produced. Dropping them would leave the
 *   evaluation panel permanently empty with nothing to explain why.
 * - **There is no "unsure" verdict**, matching the entity's own reasoning: a
 *   labeller who is unsure should *skip*, because a guessed label trains the
 *   model on noise and is indistinguishable afterwards from a confident one.
 *   Skip advances without writing anything.
 */
export function LabelingContent({ org }: { org: string }) {
  const queue = useLabelingQueue(org);
  const record = useRecordLabel(org);
  const session = useSession();
  // An ALLOWLIST, not `!== "viewer"`. `effectiveRole` returns null while the
  // session is loading, and `null !== "viewer"` is true — so the negated form
  // offered a viewer the label buttons for as long as the session took to
  // resolve. The server would still refuse, but a UI must not offer an action
  // it knows will be rejected.
  const canLabel = canRecordLabel(effectiveRole(session.data, org));
  // Skipped pairs are tracked rather than an index, because the queue is NOT a
  // stable list: the server excludes already-labelled pairs, so recording a
  // verdict makes the list itself shift down. An index advanced on success
  // therefore skips a pair — it moves forward while the list moves under it.
  // Deriving "the first pair not skipped" is correct under both operations: a
  // verdict removes its pair server-side, a skip removes it client-side, and
  // neither needs to know what the other did.
  const [skipped, setSkipped] = useState<Set<string>>(new Set());
  const [failure, setFailure] = useState<string | null>(null);

  const items = queue.data?.items ?? [];
  const remaining = items.filter((item) => !skipped.has(pairKey(item.rec_a_key, item.rec_b_key)));
  const current = nextUnskipped(items, skipped);

  const skip = useCallback(() => {
    if (!current) return;
    const key = pairKey(current.rec_a_key, current.rec_b_key);
    setSkipped((seen) => new Set(seen).add(key));
  }, [current]);

  const give = useCallback(
    (verdict: Verdict) => {
      if (!current || !canLabel) return;
      setFailure(null);
      record.mutate(labelPayloadFor(current, verdict), {
        // No advance here: the pair leaves the queue server-side, so the
        // invalidated refetch is what moves us on. Advancing as well would
        // step past the next pair.
        onError: (cause) => {
          setFailure(
            cause instanceof BffRequestError ? cause.message : "could not record the verdict",
          );
        },
      });
    },
    [canLabel, current, record],
  );

  // Keyboard first, because this screen is used in long sessions and reaching
  // for a mouse per pair is the difference between labelling 20 and 200.
  useEffect(() => {
    if (!canLabel) return undefined;
    const onKey = (event: KeyboardEvent) => {
      if (event.metaKey || event.ctrlKey || event.altKey) return;
      const target = event.target;
      // Never hijack a key meant for a field.
      if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement) return;
      if (event.key === "m") {
        event.preventDefault();
        give("match");
      } else if (event.key === "n") {
        event.preventDefault();
        give("no_match");
      } else if (event.key === "s") {
        event.preventDefault();
        skip();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
    };
  }, [canLabel, give, skip]);

  const coverage = queue.data?.coverage;

  return (
    <div className="flex w-full flex-col gap-4 lg:gap-6" data-testid="labeling-page">
      <Card data-testid="labeling-coverage">
        <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
          <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
            Label coverage
          </CardTitle>
          <CardDescription>
            What the matcher has been taught so far. A set that is almost all one verdict teaches it
            very little, which is why these are split rather than totalled.
          </CardDescription>
        </CardHeader>
        <CardContent className="p-4 pt-2 lg:p-6 lg:pt-3">
          {queue.isPending && <Skeleton className="h-10" data-testid="coverage-loading" />}
          {coverage !== undefined && (
            <div className="flex flex-wrap gap-x-8 gap-y-2">
              {[
                { label: "Total", value: coverage.total },
                { label: "Match", value: coverage.matches },
                { label: "No match", value: coverage.non_matches },
                { label: "From a human", value: coverage.by_origin.human },
                { label: "From a review", value: coverage.by_origin.reviewer },
              ].map(({ label, value }) => (
                <div key={label} className="flex flex-col gap-0.5">
                  <span className="text-muted-foreground text-xs tracking-wide uppercase">
                    {label}
                  </span>
                  <span className="font-mono text-lg tabular-nums">{value}</span>
                </div>
              ))}
            </div>
          )}
        </CardContent>
      </Card>

      <Card data-testid="labeling-queue">
        <CardHeader className="p-4 pb-1 lg:p-6 lg:pb-2">
          <CardTitle className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
            {current ? `${String(remaining.length)} left to judge` : "Nothing to label"}
          </CardTitle>
          <CardDescription>
            {queue.data === undefined ? (
              "The pairs the matcher is least sure about."
            ) : (
              <>
                Sampled from the band{" "}
                <span className="font-mono tabular-nums">
                  {queue.data.band.min.toFixed(2)}–{queue.data.band.max.toFixed(2)}
                </span>{" "}
                around the auto-merge threshold of{" "}
                <span className="font-mono tabular-nums">{queue.data.threshold.toFixed(2)}</span>. A
                verdict here is worth most where the model is closest to guessing.
              </>
            )}
          </CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-4 p-4 pt-2 lg:p-6 lg:pt-3">
          {queue.isPending && <Skeleton className="h-40" data-testid="labeling-loading" />}

          {!queue.isPending && items.length === 0 && (
            <p className="text-muted-foreground text-sm" data-testid="labeling-empty">
              No pair is waiting for a verdict — either nothing has been scored into the band yet,
              or every pair in it has already been labelled. Run an analysis to produce more.
            </p>
          )}

          {!queue.isPending && items.length > 0 && current === undefined && (
            <p className="text-muted-foreground text-sm" data-testid="labeling-exhausted">
              Every pair in this batch has been judged or skipped. Reload for the next one.
            </p>
          )}

          {current !== undefined && (
            <>
              <div className="flex flex-wrap items-baseline gap-x-6 gap-y-1">
                <div className="flex flex-col gap-0.5">
                  <span className="text-muted-foreground text-xs tracking-wide uppercase">
                    Model score
                  </span>
                  <span
                    className="font-mono text-2xl tabular-nums"
                    data-testid="labeling-probability"
                  >
                    {current.match_probability.toFixed(3)}
                  </span>
                </div>
                <p className="text-muted-foreground max-w-prose text-sm">
                  {queue.data !== undefined && current.match_probability < queue.data.threshold ? (
                    <>
                      Just below the auto-merge threshold of{" "}
                      <span className="font-mono tabular-nums">
                        {queue.data.threshold.toFixed(2)}
                      </span>
                      , so these two were left apart — that is why it is asking.
                    </>
                  ) : (
                    <>
                      At or above the threshold, so these two would be merged automatically. A
                      verdict confirms or corrects that.
                    </>
                  )}
                  {current.model_version !== null && (
                    <>
                      {" "}
                      Scored by <span className="font-mono">{current.model_version}</span>, and the
                      verdict is recorded against it.
                    </>
                  )}
                </p>
              </div>

              <FieldCompareGrid org={org} keyA={current.rec_a_key} keyB={current.rec_b_key} />

              {canLabel ? (
                <div className="flex flex-wrap items-center gap-2">
                  <Button
                    onClick={() => {
                      give("match");
                    }}
                    disabled={record.isPending}
                    data-testid="labeling-match"
                  >
                    Same entity <kbd className="ml-2 font-mono text-xs opacity-70">m</kbd>
                  </Button>
                  <Button
                    variant="outline"
                    onClick={() => {
                      give("no_match");
                    }}
                    disabled={record.isPending}
                    data-testid="labeling-no-match"
                  >
                    Different <kbd className="ml-2 font-mono text-xs opacity-70">n</kbd>
                  </Button>
                  <Button variant="ghost" onClick={skip} data-testid="labeling-skip">
                    Unsure, skip <kbd className="ml-2 font-mono text-xs opacity-70">s</kbd>
                  </Button>
                  <span className="text-muted-foreground text-xs">
                    Skipping writes nothing. A guessed verdict is indistinguishable from a confident
                    one afterwards.
                  </span>
                </div>
              ) : (
                <p className="text-muted-foreground text-sm" data-testid="labeling-readonly">
                  Recording a verdict needs the steward role. You can see what the matcher is unsure
                  about.
                </p>
              )}
            </>
          )}

          {failure !== null && (
            <p className="text-destructive text-sm" role="alert" data-testid="labeling-error">
              {failure}
            </p>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
