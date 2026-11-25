"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { bffFetch } from "@/lib/api/client";
import { queryKeys } from "./keys";

/**
 * One key as the list serves it: `ApiKeyWire`, which has no `key_hash`. The
 * plaintext is not here either — it exists only in {@link ApiKeyCreated}.
 */
export interface ApiKeyRow {
  id: string;
  org_id: string;
  role: string;
  created_at: string;
  revoked_at: string | null;
  updated_at: string;
  /** Added by the BFF: one of this app's own service keys, which cannot be
   * revoked from here. */
  service: boolean;
}

/** The mint response: the only time the plaintext `key` is ever returned. */
export interface ApiKeyCreated {
  id: string;
  key: string;
  role: string;
}

/** The roles an admin may mint, lowest first. `operator` is never mintable. */
export const MINTABLE_ROLES = ["viewer", "steward", "admin"] as const;
export type MintableRole = (typeof MINTABLE_ROLES)[number];

export function useApiKeys(org: string, enabled = true) {
  return useQuery({
    queryKey: queryKeys.apiKeys(org),
    queryFn: () => bffFetch<ApiKeyRow[]>(`/api/orgs/${org}/api-keys`),
    enabled,
  });
}

export function useCreateApiKey(org: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (role: MintableRole) =>
      bffFetch<ApiKeyCreated>(`/api/orgs/${org}/api-keys`, {
        method: "POST",
        body: JSON.stringify({ role }),
      }),
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.apiKeys(org) });
    },
  });
}

export function useRevokeApiKey(org: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (keyId: string) =>
      bffFetch<{ revoked: boolean }>(`/api/orgs/${org}/api-keys/${encodeURIComponent(keyId)}`, {
        method: "DELETE",
      }),
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.apiKeys(org) });
    },
  });
}
