"use client";

import {
  useInfiniteQuery,
  useMutation,
  useQuery,
  useQueryClient,
  type InfiniteData,
} from "@tanstack/react-query";

import { bffFetch } from "@/lib/api/client";
import { queryKeys } from "./keys";

export type Resolution = "match" | "no_match" | "dismiss";

export interface ReviewRow {
  review_id: string;
  subject_type: string;
  rec_a_key: string | null;
  rec_b_key: string | null;
  entity_id: string | null;
  reason: string;
  match_probability: number | null;
  waterfall: Record<string, unknown> | null;
  status: string;
  first_seen_run_id: string;
  last_seen_run_id: string;
}

export interface ReviewPage {
  items: ReviewRow[];
  next_cursor: string | null;
}

export interface ResolveResult {
  status: "applied" | "staged";
  assertion_id?: string | null;
  apply_job?: string;
}

const inboxKey = (org: string, reason: string | null) =>
  [...queryKeys.org(org), "review-inbox", reason ?? "all"] as const;

export function useReviewInbox(org: string, reason: string | null) {
  return useInfiniteQuery({
    queryKey: inboxKey(org, reason),
    queryFn: ({ pageParam }) => {
      const params = new URLSearchParams({ limit: "50" });
      if (reason) params.set("reason", reason);
      if (pageParam) params.set("cursor", pageParam);
      return bffFetch<ReviewPage>(`/api/orgs/${org}/reviews?${params.toString()}`);
    },
    initialPageParam: "",
    getNextPageParam: (last) => last.next_cursor ?? undefined,
  });
}

function dropReviews(
  data: InfiniteData<ReviewPage> | undefined,
  ids: ReadonlySet<string>,
): InfiniteData<ReviewPage> | undefined {
  if (!data) return data;
  return {
    ...data,
    pages: data.pages.map((page) => ({
      ...page,
      items: page.items.filter((item) => !ids.has(item.review_id)),
    })),
  };
}

/** Resolve one review. Optimistic removal from the inbox; the honest lake-state
 * chip comes from the response `status` — never guessed. */
export function useResolveReview(org: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ reviewId, resolution }: { reviewId: string; resolution: Resolution }) =>
      bffFetch<ResolveResult>(`/api/orgs/${org}/reviews/${reviewId}/resolve`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ resolution }),
      }),
    onMutate: async ({ reviewId }) => {
      await queryClient.cancelQueries({ queryKey: [...queryKeys.org(org), "review-inbox"] });
      const snapshots = queryClient.getQueriesData<InfiniteData<ReviewPage>>({
        queryKey: [...queryKeys.org(org), "review-inbox"],
      });
      for (const [key, data] of snapshots) {
        queryClient.setQueryData(key, dropReviews(data, new Set([reviewId])));
      }
      return { snapshots };
    },
    onError: (_error, _vars, context) => {
      for (const [key, data] of context?.snapshots ?? []) {
        queryClient.setQueryData(key, data);
      }
    },
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: [...queryKeys.org(org), "staged"] });
      void queryClient.invalidateQueries({ queryKey: queryKeys.metrics(org) });
    },
  });
}

export interface BulkResult {
  status: "applied" | "staged";
  results: unknown[];
  failed: number;
  apply_job?: string;
}

export function useBulkResolve(org: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: { items: { review_id: string; resolution: Resolution }[] }) =>
      bffFetch<BulkResult>(`/api/orgs/${org}/reviews/bulk-resolve`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ ...body, apply_now: true }),
      }),
    onSuccess: (_result, variables) => {
      const ids = new Set(variables.items.map((item) => item.review_id));
      const snapshots = queryClient.getQueriesData<InfiniteData<ReviewPage>>({
        queryKey: [...queryKeys.org(org), "review-inbox"],
      });
      for (const [key, data] of snapshots) {
        queryClient.setQueryData(key, dropReviews(data, ids));
      }
    },
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: [...queryKeys.org(org), "staged"] });
      void queryClient.invalidateQueries({ queryKey: queryKeys.metrics(org) });
      void queryClient.invalidateQueries({ queryKey: queryKeys.jobs(org) });
    },
  });
}

export function useStagedCount(org: string) {
  return useQuery({
    queryKey: [...queryKeys.org(org), "staged"],
    queryFn: () => bffFetch<{ pending_count: number }>(`/api/orgs/${org}/staged`),
    refetchInterval: 15_000,
    select: (data) => data.pending_count,
  });
}

export interface AssertionRow {
  assertion_id: string;
  rec_a_key: string;
  rec_b_key: string;
  kind: "always" | "never";
  active: boolean;
  created_by: string;
  created_at: string;
  retracted_by: string | null;
  retracted_at: string | null;
  note: string | null;
}

export function useAssertions(org: string, includeRetracted: boolean) {
  return useInfiniteQuery({
    queryKey: [...queryKeys.org(org), "assertions", includeRetracted],
    queryFn: ({ pageParam }) => {
      const params = new URLSearchParams({ limit: "100" });
      if (includeRetracted) params.set("include_retracted", "true");
      if (pageParam) params.set("cursor", pageParam);
      return bffFetch<{ items: AssertionRow[]; next_cursor: string | null }>(
        `/api/orgs/${org}/assertions?${params.toString()}`,
      );
    },
    initialPageParam: "",
    getNextPageParam: (last) => last.next_cursor ?? undefined,
  });
}

export function useRetractAssertion(org: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (assertionId: string) =>
      bffFetch<ResolveResult>(`/api/orgs/${org}/assertions/${assertionId}`, {
        method: "DELETE",
      }),
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: [...queryKeys.org(org), "assertions"] });
      void queryClient.invalidateQueries({ queryKey: [...queryKeys.org(org), "contradictions"] });
    },
  });
}

export interface Contradiction {
  rec_a_key: string;
  rec_b_key: string;
  never_assertion_id: string;
  always_assertion_ids: string[];
  component: string[];
}

export function useContradictions(org: string) {
  return useQuery({
    queryKey: [...queryKeys.org(org), "contradictions"],
    queryFn: () =>
      bffFetch<{ contradictions: Contradiction[] }>(`/api/orgs/${org}/assertions/contradictions`),
    select: (data) => data.contradictions,
  });
}

export interface UnmergeResult {
  status: "applied" | "staged";
  pairs: number;
  apply_job?: string;
}

export function useUnmerge(org: string, entityId: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (records: string[]) =>
      bffFetch<UnmergeResult>(`/api/orgs/${org}/golden-records/${entityId}/unmerge`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ records, apply_now: true }),
      }),
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.entity(org, entityId) });
      void queryClient.invalidateQueries({ queryKey: queryKeys.jobs(org) });
    },
  });
}
