"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { bffFetch } from "@/lib/api/client";
import type { MergeOverridePayload } from "@/lib/domain/merge-draft";
import { queryKeys } from "./keys";
import { dropReviewsFromInboxCache, type SourceRecord } from "./reviews";

/** A group member: the records-by-key shape plus the entity it belongs to
 * (null when the record has no membership row — a not-yet-resolved side). */
export interface GroupMember extends SourceRecord {
  entity_id: string | null;
}

export interface MergeGroup {
  items: GroupMember[];
  /** The union's size before the server cap — the dialog says "showing N of M". */
  total: number;
  snapshot: number;
}

/**
 * The merge dialog's group: the union of BOTH review sides' entity memberships,
 * rec_a's entity first. One server round trip — the server composes the pair →
 * entity → members → values walk, so the dialog can never drift from the
 * server's idea of what a record's values are (the same rule `useMemberValues`
 * states for the entity page).
 */
export function useMergeGroup(org: string, ds: string, reviewId: string | null) {
  return useQuery({
    queryKey: [...queryKeys.org(org), "ds", ds, "merge-group", reviewId ?? ""],
    queryFn: () =>
      bffFetch<MergeGroup>(
        `/api/orgs/${org}/datasets/${ds}/reviews/${encodeURIComponent(reviewId ?? "")}/merge-group`,
      ),
    enabled: reviewId !== null,
    staleTime: 60_000,
  });
}

export interface MergeGroupPayload {
  records: string[];
  master: string;
  /** Absent on a manual merge (spec 2.10): field corrections stay post-merge. */
  overrides?: MergeOverridePayload[];
  review_id?: string;
  /** The review dialog stages deliberately; the manual merge applies now. */
  apply_now?: boolean;
}

export interface MergeGroupResult {
  status: string;
  action_ids: string[];
  apply_job?: string;
}

/**
 * Submit the dialog's merge: the assertion star + master protect + the staged
 * overrides, as one bundle. Deliberately NOT `apply_now` — the inbox's other
 * resolutions stage too, and the staged banner is how the steward knows.
 */
export function useMergeGroupSubmit(org: string, ds: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (payload: MergeGroupPayload) =>
      bffFetch<MergeGroupResult>(`/api/orgs/${org}/datasets/${ds}/merges`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(payload),
      }),
    onSuccess: (_result, payload) => {
      // The bundle resolves the review as a match — drop it from the inbox
      // optimistically, the same way a verdict does, rather than waiting out
      // the refetch with a decided review still showing.
      if (payload.review_id !== undefined) {
        dropReviewsFromInboxCache(queryClient, org, new Set([payload.review_id]));
      }
      void queryClient.invalidateQueries({ queryKey: [...queryKeys.org(org), "review-inbox"] });
      // The merged entity's golden changed either way (review dialog or the
      // records browser's manual selection) — refresh both read surfaces.
      void queryClient.invalidateQueries({
        queryKey: [...queryKeys.ds(org, ds), "golden-records"],
      });
      void queryClient.invalidateQueries({ queryKey: [...queryKeys.ds(org, ds), "entity"] });
      // The staged banner's key is org-rooted (no ds segment) — mirror
      // `useResolveReview`'s settled invalidations rather than inventing one.
      void queryClient.invalidateQueries({ queryKey: [...queryKeys.org(org), "staged"] });
      void queryClient.invalidateQueries({ queryKey: queryKeys.metrics(org, ds) });
    },
  });
}
