"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { bffFetch } from "@/lib/api/client";
import type { PairLabel } from "@/lib/entities/bindings/PairLabel";
import { queryKeys } from "./keys";

/**
 * One pair the model is unsure about.
 *
 * `model_version` and `score_at_label` travel with the item and must be sent
 * back when the verdict is recorded — they are what makes the label usable by
 * per-version evaluation, which can only measure labels carrying the score that
 * version actually produced.
 */
export interface LabelCandidate {
  rec_a_key: string;
  rec_b_key: string;
  match_probability: number;
  /** The model that produced the score, e.g. `v0001`. Null on an old row. */
  model_version: string | null;
  run_id: string | null;
  scored_at: string | null;
}

/** Coverage split by verdict and origin — a total alone hides the imbalance. */
export interface LabelCoverage {
  total: number;
  matches: number;
  non_matches: number;
  by_origin: { human: number; reviewer: number; llm: number };
}

export interface LabelingQueue {
  items: LabelCandidate[];
  snapshot: number;
  /** The auto-merge threshold the band is sampled around. */
  threshold: number;
  band: { min: number; max: number };
  coverage: LabelCoverage;
}

/** The verdicts a labeller can give. A dismissal is not one of them. */
export type Verdict = "match" | "no_match";

export interface LabelInput {
  rec_a_key: string;
  rec_b_key: string;
  verdict: Verdict;
  model_version?: string | null;
  score_at_label?: number | null;
  note?: string;
}

/** A pair's identity, in the order the queue gives it. */
export function pairKey(recA: string, recB: string): string {
  return `${recA}|${recB}`;
}

/**
 * The next pair to ask about: the first the labeller has not skipped.
 *
 * Derived rather than tracked as an index, because the queue is **not a stable
 * list** — the server excludes already-labelled pairs, so recording a verdict
 * makes the list shift down. An index advanced on a successful label therefore
 * moves forward while the list moves under it, and silently skips a pair. This
 * is correct under both operations: a verdict removes its pair server-side, a
 * skip removes it client-side, and neither needs to know what the other did.
 */
export function nextUnskipped(
  items: readonly LabelCandidate[],
  skipped: ReadonlySet<string>,
): LabelCandidate | undefined {
  return items.find((item) => !skipped.has(pairKey(item.rec_a_key, item.rec_b_key)));
}

/**
 * Whether a role may record a verdict.
 *
 * An allowlist, deliberately. `effectiveRole` returns `null` while the session
 * is loading, so the obvious `role !== "viewer"` is true for a *loading*
 * session and offers a viewer the buttons until it settles. The server refuses
 * either way, but a UI must not offer an action it knows will be rejected.
 */
export function canRecordLabel(role: "viewer" | "steward" | "admin" | null): boolean {
  return role === "steward" || role === "admin";
}

/**
 * The verdict to send for a queue item.
 *
 * **`model_version` and `score_at_label` come from the item and are never
 * omitted.** They are nullable on the entity, so a label without them is still
 * valid training signal — but it is invisible to per-version evaluation, which
 * can only measure labels carrying the score that version actually produced.
 * Dropping them here would leave
 * `/models/{name}/versions/{v}/evaluation` reporting "no label can be scored"
 * forever, with nothing anywhere to explain why. That is the whole reason this
 * is a function with a test rather than an object literal inside a click
 * handler.
 */
export function labelPayloadFor(item: LabelCandidate, verdict: Verdict): LabelInput {
  return {
    rec_a_key: item.rec_a_key,
    rec_b_key: item.rec_b_key,
    verdict,
    model_version: item.model_version,
    score_at_label: item.match_probability,
  };
}

/**
 * The pairs worth asking about, uncertainty-sampled around the threshold.
 *
 * Already-labelled pairs are excluded server-side, so re-fetching after a
 * verdict yields a queue that has genuinely moved on rather than one the client
 * has to filter.
 */
export function useLabelingQueue(org: string) {
  return useQuery({
    queryKey: queryKeys.labelingQueue(org),
    queryFn: () => bffFetch<LabelingQueue>(`/api/orgs/${org}/labeling/next?limit=25`),
  });
}

/** Every verdict recorded, newest first. */
export function useLabels(org: string) {
  return useQuery({
    queryKey: queryKeys.labels(org),
    queryFn: () => bffFetch<PairLabel[]>(`/api/orgs/${org}/labels?limit=50`),
  });
}

/**
 * Record a verdict.
 *
 * Invalidates the queue as well as the label list: the server excludes
 * already-labelled pairs, so the queue's contents change as a result of this
 * call and a stale one would offer the same pair again.
 */
export function useRecordLabel(org: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: LabelInput) =>
      bffFetch<PairLabel>(`/api/orgs/${org}/labels`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
      }),
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.labelingQueue(org) });
      void queryClient.invalidateQueries({ queryKey: queryKeys.labels(org) });
    },
  });
}
