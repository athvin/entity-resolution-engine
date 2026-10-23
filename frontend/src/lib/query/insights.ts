"use client";

import { useQuery } from "@tanstack/react-query";

import { bffFetch } from "@/lib/api/client";
import { queryKeys } from "./keys";

/**
 * One retained resolution-metric snapshot, plus the two rates the server
 * derives on read.
 *
 * The rates are `number | null` rather than `number` and that is the whole
 * point of O7a's third judgement: **no records is not a 0% duplicate rate, it
 * is no answer.** Reporting 0.0 would draw a reassuring flat line for an empty
 * corpus, so the server sends null and every reader here has to say "—".
 */
export interface InsightRow {
  run_id: string;
  captured_at: string;
  records: number;
  entities: number;
  duplicate_records: number;
  duplicate_groups: number;
  largest_group: number;
  review_backlog: number;
  auto_merge_pairs: number;
  review_band_pairs: number;
  /** Derived on read: `duplicate_records / records`, or null on an empty corpus. */
  duplicate_rate: number | null;
  /** Derived on read: `records / entities`, or null when nothing is resolved. */
  records_per_entity: number | null;
}

/**
 * The series, newest-first as the server pages it.
 *
 * Kept in the server's order rather than reversed here: the table reads
 * newest-first the way a run list does, and the sparkline reverses for itself
 * because a trend reads left-to-right in time.
 */
export function useInsights(org: string) {
  return useQuery({
    queryKey: queryKeys.insights(org),
    queryFn: () => bffFetch<InsightRow[]>(`/api/orgs/${org}/insights`),
  });
}

/** One source's contribution to the corpus's duplicates, for one run. */
export interface SourceAttribution {
  run_id: string;
  source_system: string;
  captured_at: string;
  records: number;
  duplicate_records: number;
  /**
   * How many duplicate groups this source has a record in.
   *
   * Its own number beside `duplicate_records` on purpose: a source can account
   * for few duplicate RECORDS while touching many groups, which is the
   * signature of a feed that adds one bad row to everything rather than one bad
   * batch.
   */
  contributed_groups: number;
  /** Derived on read; null for a source with no records at all. */
  duplicate_rate: number | null;
}

export interface SourceAttributionPage {
  snapshot: number;
  /** Which run the attribution describes — the newest unless one was named. */
  run_id: string | null;
  /** Worst first: the question is "which feed is causing this". */
  sources: SourceAttribution[];
}

export function useSourceAttribution(org: string) {
  return useQuery({
    queryKey: queryKeys.insightSources(org),
    queryFn: () => bffFetch<SourceAttributionPage>(`/api/orgs/${org}/insights/sources`),
  });
}
