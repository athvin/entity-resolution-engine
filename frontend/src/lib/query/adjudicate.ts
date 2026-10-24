"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { bffFetch } from "@/lib/api/client";
import { queryKeys } from "./keys";

/** One stored recommendation, mirroring the BFF's `StoredRecommendation`. */
export interface ReviewRecommendation {
  review_id: string;
  verdict: string;
  confidence: number;
  reasoning: string;
  model: string;
  match_probability: number | null;
  requested_by: string;
  created_at: string;
}

/**
 * The recommendations for a page of reviews, keyed by review id.
 *
 * One request for the whole page rather than one per row — the inbox renders a
 * page at a time and a request per row would be a storm for data that is
 * decorative until someone looks at it.
 *
 * The key includes the sorted id list so a different page is a different cache
 * entry; sorted because the inbox's order is not stable across refetches and an
 * order-sensitive key would miss a cache it should have hit.
 */
export function useReviewRecommendations(org: string, reviewIds: string[]) {
  const ids = [...new Set(reviewIds)].sort();
  return useQuery({
    queryKey: queryKeys.reviewRecommendations(org, ids),
    queryFn: () => {
      const params = new URLSearchParams();
      for (const id of ids) params.append("review_id", id);
      return bffFetch<Record<string, ReviewRecommendation>>(
        `/api/orgs/${org}/reviews/recommendations?${params.toString()}`,
      );
    },
    enabled: ids.length > 0,
  });
}

export function useRecommendReview(org: string) {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (reviewId: string) =>
      bffFetch<ReviewRecommendation>(
        `/api/orgs/${org}/reviews/${encodeURIComponent(reviewId)}/recommend`,
        { method: "POST" },
      ),
    // Invalidate the whole family rather than patching one cache entry: the key
    // carries the page's id list, so the entry this row belongs to is not
    // knowable from the row alone.
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: queryKeys.reviewRecommendationsAll(org) });
    },
  });
}
