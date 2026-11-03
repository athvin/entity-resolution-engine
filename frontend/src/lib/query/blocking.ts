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
  /**
   * Whether this rule's largest blocks could be enumerated. False for a rule
   * carrying a filter condition, which has no block breakdown at all — which
   * must render differently from "examined, no large blocks found".
   */
  has_block_breakdown: boolean;
}

export interface LargestBlockEstimate {
  /** Which rule this block belongs to; every rule is broken down, not just the first. */
  match_key: number;
  key_values: string[];
  /** The pairs a run SCORES in this block, under the model's link type. */
  pairs: number;
  /** Rows carrying this key value. `pairs` is quadratic in this. */
  rows: number;
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
export function useBlockingEstimate(org: string, ds: string, enabled: boolean) {
  return useQuery({
    queryKey: queryKeys.blockingEstimate(org, ds),
    queryFn: () =>
      bffFetch<BlockingEstimate>(`/api/orgs/${org}/datasets/${ds}/blocking-estimate?largest=5`),
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

export function useConfigSegments(org: string, ds: string) {
  return useQuery({
    queryKey: queryKeys.configSegments(org, ds),
    queryFn: async () => {
      const body = await bffFetch<{ segments?: SegmentChoice[] }>(
        `/api/orgs/${org}/datasets/${ds}/config/segments`,
      );
      // The envelope is the server's `{segments: […]}`; unwrapped here rather
      // than asserted about, which is the W0.5b lesson.
      return body.segments ?? [];
    },
  });
}
