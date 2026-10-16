"use client";

import { useMutation } from "@tanstack/react-query";

import { bffFetch } from "@/lib/api/client";

export interface EnrichSuggestion {
  field: string;
  value: string;
  reason: string;
}

export interface EnrichResponse {
  suggestions: EnrichSuggestion[];
  refused: boolean;
}

/**
 * Ask the AI for fill/normalize suggestions on one entity (PR 2.3). Read-only:
 * this returns suggestions; accepting one is a separate `useMassUpdate` write
 * (the existing `override_field` path). Not auto-fetched — a steward triggers
 * it, because it spends model tokens.
 */
export function useEnrich(org: string) {
  return useMutation({
    mutationFn: (entityId: string) =>
      bffFetch<EnrichResponse>(`/api/orgs/${org}/enrich`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ entity_id: entityId }),
      }),
  });
}
