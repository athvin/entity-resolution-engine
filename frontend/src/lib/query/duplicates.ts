"use client";

import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { bffFetch } from "@/lib/api/client";
import { queryKeys } from "./keys";

export interface DuplicateGroup {
  entity_id: string;
  member_count: number;
  members: string[];
  given_name: string | null;
  family_name: string | null;
  email: string | null;
}

export interface DuplicatesPage {
  items: DuplicateGroup[];
  snapshot: number;
  next_cursor: string | null;
}

export function useDuplicates(org: string) {
  return useInfiniteQuery({
    queryKey: [...queryKeys.org(org), "duplicates"],
    queryFn: ({ pageParam }) => {
      const params = new URLSearchParams({ limit: "50" });
      if (pageParam) params.set("cursor", pageParam);
      return bffFetch<DuplicatesPage>(`/api/orgs/${org}/duplicates?${params.toString()}`);
    },
    initialPageParam: "",
    getNextPageParam: (last) => last.next_cursor ?? undefined,
  });
}

export interface MatchScoreRow {
  rec_a_key: string;
  rec_b_key: string;
  match_probability: number;
  model_version: string;
  evidence: Record<string, unknown> | null;
  scored_at: string | null;
}

/**
 * Scored pairs inside a probability band — the audit read over auto-merge's
 * own evidence (design §5.3: "show me everything merged between .95 and .97").
 */
export function useMatchScores(org: string, band: { low: number; high: number }) {
  return useQuery({
    queryKey: [...queryKeys.org(org), "match-scores", band.low, band.high],
    queryFn: () => {
      const params = new URLSearchParams({
        limit: "200",
        band_low: String(band.low),
        band_high: String(band.high),
      });
      return bffFetch<{ items: MatchScoreRow[]; snapshot: number }>(
        `/api/orgs/${org}/match-scores?${params.toString()}`,
      );
    },
    staleTime: 30_000,
  });
}

export interface AssertionWrite {
  status: "applied" | "staged";
  assertion_id?: string;
}

/** Write one durable always/never assertion for an ad-hoc pair. */
export function useAddAssertion(org: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: { kind: "always" | "never"; a: string; b: string; note?: string }) =>
      bffFetch<AssertionWrite>(`/api/orgs/${org}/assertions`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ ...body, apply_now: true }),
      }),
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: [...queryKeys.org(org), "assertions"] });
      void queryClient.invalidateQueries({ queryKey: [...queryKeys.org(org), "duplicates"] });
      void queryClient.invalidateQueries({ queryKey: [...queryKeys.org(org), "staged"] });
      void queryClient.invalidateQueries({ queryKey: queryKeys.metrics(org) });
      void queryClient.invalidateQueries({ queryKey: queryKeys.jobs(org) });
    },
  });
}
