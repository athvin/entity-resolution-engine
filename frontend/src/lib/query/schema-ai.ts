"use client";

import { useMutation } from "@tanstack/react-query";

import { bffFetch } from "@/lib/api/client";
import type { EntityType } from "@/lib/domain/csv-profile";
import type { MappingSuggestion, SchemaProfile } from "@/lib/schema-ai/redact";

export interface SuggestMappingInput extends SchemaProfile {
  /** Columns the heuristic already claimed, so the model fills gaps only. */
  taken_columns: string[];
  taken_attributes: string[];
}

/**
 * Ask the model to map the columns the regex heuristic left unassigned.
 *
 * A mutation rather than a query even though it writes nothing: it is an
 * explicit act with a token cost, and a `useQuery` would fire it on mount for
 * every file a steward previews.
 *
 * No cache invalidation — the answer is applied to local wizard state, and
 * there is no server-side resource to refresh.
 */
export function useSuggestMapping(org: string, ds: string) {
  return useMutation({
    mutationFn: (input: SuggestMappingInput) =>
      bffFetch<{ mappings: MappingSuggestion[] }>(
        `/api/orgs/${org}/datasets/${ds}/sources/suggest-mapping`,
        {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify(input),
        },
      ),
  });
}

export type { EntityType, MappingSuggestion };
