"use client";

import { useQuery } from "@tanstack/react-query";

import { bffFetch } from "@/lib/api/client";
import { queryKeys } from "./keys";

/** One blocking rule's contribution, in the model's own rule order. */
export interface BlockingRuleEstimate {
  blocking_rule: string;
  match_key: number;
  /** Pairs this rule adds that no earlier rule already generated. */
  marginal_pairs: number;
  cumulative_pairs: number;
}

export interface LargestBlockEstimate {
  key_values: string[];
  pairs: number;
}

/**
 * What a run would cost. Field names are the server's `BlockingEstimate`, not
 * this file's preference.
 */
export interface BlockingEstimate {
  /** Every pair the corpus could produce — the cost with no blocking at all. */
  total_possible_pairs: number;
  /** What the declared rules actually generate: the number that decides cost. */
  candidate_pairs: number;
  per_rule: BlockingRuleEstimate[];
  largest_blocks: LargestBlockEstimate[];
}

/**
 * On demand only. The estimate opens the org's lake and counts pairs, so
 * fetching it on every Runs page load would make a cheap page expensive —
 * `enabled` is the caller's explicit "yes, estimate it now".
 */
export function useBlockingEstimate(org: string, enabled: boolean) {
  return useQuery({
    queryKey: queryKeys.blockingEstimate(org),
    queryFn: () => bffFetch<BlockingEstimate>(`/api/orgs/${org}/blocking-estimate?largest=5`),
    enabled,
    // The corpus does not change while a page is open, and the query is
    // expensive; a stale estimate is better than a surprise scan.
    staleTime: 5 * 60_000,
    retry: false,
  });
}

/** A named slice of the corpus from the published config's `segments:` block. */
export interface SegmentChoice {
  name: string;
  predicate: string;
}

export function useConfigSegments(org: string) {
  return useQuery({
    queryKey: queryKeys.configSegments(org),
    queryFn: async () => {
      const body = await bffFetch<{ segments?: SegmentChoice[] }>(
        `/api/orgs/${org}/config/segments`,
      );
      // The envelope is the server's `{segments: […]}`; unwrapped here rather
      // than asserted about, which is the W0.5b lesson.
      return body.segments ?? [];
    },
  });
}
