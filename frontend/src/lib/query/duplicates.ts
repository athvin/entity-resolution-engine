"use client";

import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { bffFetch } from "@/lib/api/client";
import { queryKeys } from "./keys";

export interface DuplicateGroup {
  entity_id: string;
  member_count: number;
  members: string[];
  /** The group's confidence: the average of every member's attachment to the
   * group, master included, computed once server-side and displayed verbatim.
   * Null when the group carries no scored or asserted evidence. */
  group_confidence?: number | null;
  /** The strongest-attached member — the data-derived master. */
  master_record_key?: string | null;
  /** Per-member attachment confidence, keyed by record key. */
  member_confidences?: Record<string, number>;
  // Person preview fields (present for person-entity orgs).
  given_name?: string | null;
  family_name?: string | null;
  email?: string | null;
  // Account preview fields (present for account-entity orgs).
  company_name?: string | null;
  domain?: string | null;
}

export interface DuplicatesPage {
  items: DuplicateGroup[];
  snapshot: number;
  next_cursor: string | null;
}

export function useDuplicates(org: string, ds: string) {
  return useInfiniteQuery({
    queryKey: [...queryKeys.org(org), "duplicates"],
    queryFn: ({ pageParam }) => {
      const params = new URLSearchParams({ limit: "50" });
      if (pageParam) params.set("cursor", pageParam);
      return bffFetch<DuplicatesPage>(
        `/api/orgs/${org}/datasets/${ds}/duplicates?${params.toString()}`,
      );
    },
    initialPageParam: "",
    getNextPageParam: (last) => last.next_cursor ?? undefined,
  });
}

export interface MatchScoreRow {
  rec_a_key: string;
  rec_b_key: string;
  match_probability: number;
  /** The server's 1–100 display projection of `match_probability`. */
  confidence?: number;
  model_version: string;
  evidence: Record<string, unknown> | null;
  scored_at: string | null;
}

/**
 * Scored pairs inside a probability band — the audit read over auto-merge's
 * own evidence (design §5.3: "show me everything merged between .95 and .97").
 */
export function useMatchScores(org: string, ds: string, band: { low: number; high: number }) {
  return useQuery({
    queryKey: [...queryKeys.org(org), "match-scores", band.low, band.high],
    queryFn: () => {
      const params = new URLSearchParams({
        limit: "200",
        band_low: String(band.low),
        band_high: String(band.high),
      });
      return bffFetch<{ items: MatchScoreRow[]; snapshot: number }>(
        `/api/orgs/${org}/datasets/${ds}/match-scores?${params.toString()}`,
      );
    },
    staleTime: 30_000,
  });
}

export interface MatchPreviewPair {
  rec_a_key: string;
  rec_b_key: string;
  match_probability: number;
  /** The server's 1–100 display projection of `match_probability`. */
  confidence?: number;
  evidence: Record<string, unknown> | null;
}

export interface MatchPreviewResult {
  total: number;
  would_auto_merge: number;
  would_review: number;
  sample: MatchPreviewPair[];
}

/**
 * Score-only match preview (frontend-design §7.16): run the committed model over
 * the current corpus and see what it WOULD match, bucketed by the config
 * thresholds. Read-only on the backend (writes nothing). Gated behind `enabled`
 * so the (heavy) scoring runs only when the steward asks for it.
 */
export function useMatchPreview(org: string, ds: string, enabled: boolean) {
  return useQuery({
    queryKey: [...queryKeys.org(org), "match-preview"],
    queryFn: () =>
      bffFetch<MatchPreviewResult>(`/api/orgs/${org}/datasets/${ds}/match-preview?limit=50`),
    enabled,
    staleTime: 30_000,
  });
}

export interface AssertionWrite {
  status: "applied" | "staged";
  assertion_id?: string;
}

/** Write one durable always/never assertion for an ad-hoc pair. */
export function useAddAssertion(org: string, ds: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: { kind: "always" | "never"; a: string; b: string; note?: string }) =>
      bffFetch<AssertionWrite>(`/api/orgs/${org}/datasets/${ds}/assertions`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ ...body, apply_now: true }),
      }),
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: [...queryKeys.org(org), "assertions"] });
      void queryClient.invalidateQueries({ queryKey: [...queryKeys.org(org), "duplicates"] });
      void queryClient.invalidateQueries({ queryKey: [...queryKeys.org(org), "staged"] });
      void queryClient.invalidateQueries({ queryKey: queryKeys.metrics(org, ds) });
      void queryClient.invalidateQueries({ queryKey: queryKeys.jobs(org, ds) });
    },
  });
}
