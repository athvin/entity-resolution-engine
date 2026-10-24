"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { bffFetch } from "@/lib/api/client";
import { queryKeys } from "./keys";

/**
 * The tenant's AI settings as the BFF reports them.
 *
 * Mirrors `AiSettingsView` and deliberately has no key field: the stored key is
 * never returned, so there is nothing here that could render it by accident.
 * `enabled` is already the EFFECTIVE value — the BFF resolves the
 * no-row-means-inherit case — so the form can bind to it directly rather than
 * reimplementing that precedence in the browser.
 */
export interface AiSettings {
  enabled: boolean;
  model: string | null;
  key_set: boolean;
  key_set_at: string | null;
  configured: boolean;
  updated_by: string | null;
  updated_at: string | null;
  deployment_has_key: boolean;
}

/** What the form sends. Absent fields are left alone; `null` clears. */
export interface AiSettingsUpdate {
  enabled: boolean;
  model?: string | null;
  api_key?: string | null;
}

export function useAiSettings(org: string) {
  return useQuery({
    queryKey: queryKeys.aiSettings(org),
    queryFn: () => bffFetch<AiSettings>(`/api/orgs/${org}/ai-settings`),
  });
}

export function useUpdateAiSettings(org: string) {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (update: AiSettingsUpdate) =>
      bffFetch<AiSettings>(`/api/orgs/${org}/ai-settings`, {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(update),
      }),
    // The PUT returns the new state, so seed the cache from the response rather
    // than invalidating: a refetch would re-read a row the write just produced
    // and flash the old value in between.
    onSuccess: (settings) => {
      client.setQueryData(queryKeys.aiSettings(org), settings);
    },
  });
}
