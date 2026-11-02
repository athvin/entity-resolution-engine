"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { bffFetch } from "@/lib/api/client";
import type { ViewDefinition, ViewSurface } from "@/lib/views/define";
import { queryKeys } from "./keys";

export interface SavedView {
  id: string;
  org: string;
  surface: string;
  name: string;
  definition: ViewDefinition;
  created_by?: string;
  createdBy?: string;
}

export function useSavedViews(org: string, ds: string, surface: ViewSurface) {
  return useQuery({
    queryKey: queryKeys.savedViews(org, ds, surface),
    queryFn: () =>
      bffFetch<SavedView[]>(`/api/orgs/${org}/datasets/${ds}/views?surface=${surface}`),
  });
}

export function useSaveView(org: string, ds: string, surface: ViewSurface) {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (input: { name: string; definition: ViewDefinition }) =>
      bffFetch<SavedView>(`/api/orgs/${org}/datasets/${ds}/views`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ surface, ...input }),
      }),
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: queryKeys.savedViews(org, ds, surface) });
    },
  });
}

export function useDeleteView(org: string, ds: string, surface: ViewSurface) {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (viewId: string) =>
      bffFetch<{ deleted: string }>(
        `/api/orgs/${org}/datasets/${ds}/views/${encodeURIComponent(viewId)}`,
        {
          method: "DELETE",
        },
      ),
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: queryKeys.savedViews(org, ds, surface) });
    },
  });
}
