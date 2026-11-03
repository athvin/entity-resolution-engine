"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { bffFetch } from "@/lib/api/client";
import { queryKeys } from "./keys";

/**
 * Which corpus a profile describes. The same three the engine accepts, and the
 * reason one screen serves all three: the mechanics are identical, so a golden
 * profile is the raw profile with a different FROM clause.
 */
export type ProfileSubject = "raw" | "standardized" | "golden";

/** Field names below are the server's, not this file's preference. */

/**
 * One column's shape. Mirrors the `column_profile` row the server serializes
 * verbatim, which is why it is checked against the generated binding in
 * `tests/unit/wire-contract.test.ts` rather than trusted.
 *
 * Five fields are `null` for a viewer and populated for a steward — see
 * `er_server::read::VALUE_BEARING_COLUMN_FIELDS`. They are nulled rather than
 * removed, so every one is declared `| null` here and the shape does not depend
 * on who asked.
 */
export interface ColumnProfileRow {
  column_name: string;
  /** Rows the profile measured — equal to `total_row_count` unless sampled. */
  row_count: number;
  total_row_count: number;
  non_null_count: number;
  /**
   * Values that are present but empty. Separate from null on purpose: ingest
   * stores every delivered cell as a JSON string, so an empty cell is `''` and
   * not NULL, and a fill rate counting `''` as filled is a lie.
   */
  blank_count: number;
  /** HyperLogLog. Wrong in both directions — label it approximate, never exact. */
  distinct_approx: number;
  /** Exact, and only present when the profile was asked for it. */
  distinct_exact: number | null;
  /** Distinct after lower + trim + whitespace collapse. Far below
   * `distinct_exact` means the column needs standardising before it is matched on. */
  distinct_normalized: number | null;
  len_min: number | null;
  len_max: number | null;
  len_avg: number | null;
  len_p50: number | null;
  len_p95: number | null;
  /** Gated: the lexically smallest value, verbatim. */
  lex_min: string | null;
  /** Gated: the lexically largest value, verbatim. */
  lex_max: string | null;
  /** Gated: the numeric minimum, verbatim. */
  num_min: number | null;
  /** Gated: the numeric maximum, verbatim. */
  num_max: number | null;
  num_avg: number | null;
  num_std: number | null;
  cast_bigint: number;
  cast_double: number;
  cast_date: number;
  cast_timestamp: number;
  cast_boolean: number;
  entropy_bits: number | null;
  /** Gated per entry: `value` is null for a viewer, counts always survive. */
  top_values: TopValue[] | null;
}

export interface TopValue {
  value: string | null;
  count: number;
  share: number;
  rank: number;
}

/** A deterministic finding. `column_name` is null for a corpus-level one. */
export interface ProfileFindingRow {
  code: string;
  severity: "error" | "warning" | "info";
  column_name: string | null;
  message: string;
  detail: unknown;
}

/**
 * What one column would cost as a blocking key, measured with no model —
 * cheapest first, because `ORDER BY expected_pairs` is the ranking itself.
 */
export interface CandidateKeyRow {
  /** Split from the stored chr(31)-joined form by the server. */
  key_columns: string[];
  row_count: number;
  /** Rows with a usable value. `row_count - covered_rows` can never be matched
   * by a rule keyed on this column. */
  covered_rows: number;
  distinct_values: number;
  /** `Σ C(n_v, 2)` — the exact pair count an equi-join on this key produces. */
  expected_pairs: number;
  largest_block_pairs: number;
  largest_block_rows: number;
  entropy_bits: number;
  /** `1 − (covered/rows)²`: the share of pairs this key silently discards. */
  pair_loss_from_nulls: number;
}

/** A corpus nobody has profiled. Not an error — the answer is "not yet". */
export interface ProfileAbsent {
  profiled: false;
  subject: string;
  tier: number;
}

export interface ProfilePresent {
  profiled: true;
  profile_run_id: string;
  subject: string;
  source_system: string;
  tier: number;
  /** The DuckLake version profiled. Provenance only — never the cache key. */
  data_snapshot: number;
  corpus_digest: string;
  row_count: number;
  column_count: number;
  ingest_batch_id: string | null;
  config_hash: string | null;
  model_version: string | null;
  /** Travels with the proportion so a number can be labelled approximate: a
   * figure from a 5% sample shown as exact is worse than no figure. */
  sampled: boolean;
  sample_proportion: number | null;
  sample_seed: number | null;
  duration_ms: number;
  captured_at: string;
  columns: ColumnProfileRow[];
  findings: ProfileFindingRow[];
  /** Empty on a tier-1 profile, which measures no keys. */
  candidate_keys: CandidateKeyRow[];
}

export type Profile = ProfileAbsent | ProfilePresent;

/** Narrowing helper, so components branch on one named thing. */
export function isProfiled(profile: Profile | undefined): profile is ProfilePresent {
  return profile?.profiled === true;
}

/**
 * The cached profile for one subject and tier.
 *
 * Cheap — it reads three lake tables a job already wrote, so unlike
 * `useBlockingEstimate` this needs no `enabled` gate and no long `staleTime`.
 * A profile is immutable for a given corpus, so what little staleness there is
 * cannot be wrong, only old.
 */
export function useProfile(
  org: string,
  ds: string,
  subject: ProfileSubject,
  source: string | null,
  tier: number,
) {
  const params = new URLSearchParams({ subject, tier: String(tier) });
  if (source) params.set("source", source);
  return useQuery({
    queryKey: queryKeys.profile(org, ds, subject, source, tier),
    queryFn: () =>
      bffFetch<Profile>(`/api/orgs/${org}/datasets/${ds}/profile?${params.toString()}`),
    retry: false,
  });
}

/**
 * Queue a profile. Returns the job id so the caller can follow it with the
 * existing job polling rather than inventing a second progress channel.
 */
export function useRunProfile(org: string, ds: string) {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (body: { subject: ProfileSubject; source?: string; tier: number }) =>
      bffFetch<{ job_id: string }>(`/api/orgs/${org}/datasets/${ds}/profile`, {
        method: "POST",
        body: JSON.stringify(body),
      }),
    onSuccess: () => {
      // The jobs list drives the interstitial; the profile itself is only worth
      // refetching once the job is terminal, which the screen handles.
      void client.invalidateQueries({ queryKey: queryKeys.jobs(org, ds) });
    },
  });
}

/** The model's briefing over a profile, plus any draft blocking rules. */
export interface ProfileNarrative {
  summary: string;
  blocking: string[];
}

/**
 * Ask for the narrative. A mutation rather than a query, because it is a
 * REQUEST: it costs a model call, so nothing should fire it on render, and
 * re-asking is an explicit act rather than a cache miss.
 */
export function useProfileNarrative(org: string, ds: string) {
  return useMutation({
    mutationFn: (body: { subject: ProfileSubject; source?: string; tier: number }) =>
      bffFetch<ProfileNarrative>(`/api/orgs/${org}/datasets/${ds}/profile/explain`, {
        method: "POST",
        body: JSON.stringify(body),
      }),
  });
}
