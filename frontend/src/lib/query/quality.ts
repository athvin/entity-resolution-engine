"use client";

import { useQuery } from "@tanstack/react-query";

import { bffFetch } from "@/lib/api/client";
import { queryKeys } from "./keys";

/**
 * One of the worst-scoring records — a steward's work queue, not a statistic.
 *
 * Mirrors a `record_quality` row, so it is checked against the generated
 * binding in `tests/unit/wire-contract.test.ts`. That entity had
 * `languages: [rust]` and no wire until this consumer existed, which is the
 * same sequence `ingest_batch` went through in E9b: the row had been on the
 * JSON wire for as long as the endpoint had, with nothing able to check a UI
 * type against it.
 */
export interface WorstRecord {
  record_key: string;
  score: number | null;
  completeness: number | null;
  validity: number | null;
  freshness: number | null;
}

/** A tenant whose config declares no quality scoring. Not an error. */
export interface RecordQualityAbsent {
  configured: false;
  scored: number;
}

export interface RecordQualityPresent {
  configured: true;
  scored: number;
  mean_score: number | null;
  min_score: number | null;
  max_score: number | null;
  /** Three fixed cuts, as the server defines them — not re-bucketed here. */
  buckets: { below_50: number; between_50_80: number; at_least_80: number };
  /** Per-dimension means. Each is null when nothing scored that dimension. */
  dimensions: {
    completeness: number | null;
    validity: number | null;
    freshness: number | null;
  };
  worst: WorstRecord[];
}

export type RecordQuality = RecordQualityAbsent | RecordQualityPresent;

export function isScored(quality: RecordQuality | undefined): quality is RecordQualityPresent {
  return quality?.configured === true;
}

/**
 * Per-record quality: the score distribution and the worst records.
 *
 * `GET …/quality` has been implemented upstream — buckets, per-dimension means
 * and a bounded worst-records list — with **no BFF route, no hook and no
 * component** since it shipped. This is the consumer.
 *
 * Distinct from `useDataQuality`, which reads `…/data-quality`: that one feeds
 * the dashboard's completeness-by-attribute meter and is about ATTRIBUTES. This
 * is about RECORDS, and the difference is the whole value of it — a corpus can
 * be 95% complete per attribute and still have two hundred records that are
 * individually unusable.
 */
export function useRecordQuality(org: string, ds: string, limit = 20) {
  return useQuery({
    queryKey: queryKeys.recordQuality(org, ds, limit),
    queryFn: () =>
      bffFetch<RecordQuality>(`/api/orgs/${org}/datasets/${ds}/quality?limit=${String(limit)}`),
    retry: false,
  });
}
