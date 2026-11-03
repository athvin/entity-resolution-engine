"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { bffFetch } from "@/lib/api/client";
import { queryKeys } from "./keys";

/**
 * The wire shape of `dataset_json` — the control-plane fields only. The row's
 * lake env, config path and ids never cross the API: a dataset is addressed by
 * `(org, name)` and everything inside it by the dataset-scoped routes.
 */
export interface DatasetSummary {
  name: string;
  display_name: string | null;
  entity_type: "person" | "account";
  state: "provisioning" | "active" | "suspended" | "purging";
  active_config_version: number | null;
  created_at: string;
  updated_at: string;
}

/**
 * The org's datasets, for the switcher.
 *
 * Held fresh for a minute: the top bar reads this on every tenant screen, so
 * without a stale window each navigation refetches the same rarely-changing
 * list. Mutations in this file invalidate the key outright, so a create,
 * suspend or resume is still reflected immediately.
 */
export function useDatasets(org: string) {
  return useQuery({
    queryKey: queryKeys.datasets(org),
    queryFn: () => bffFetch<DatasetSummary[]>(`/api/orgs/${org}/datasets`),
    staleTime: 60_000,
  });
}

/**
 * One dataset's detail, for the `[ds]` guard. While the dataset is
 * provisioning the guard polls this until it turns active, so the interval is
 * conditional rather than always-on.
 */
export function useDataset(org: string, ds: string) {
  return useQuery({
    queryKey: queryKeys.dataset(org, ds),
    queryFn: () => bffFetch<DatasetSummary>(`/api/orgs/${org}/datasets/${ds}`),
    refetchInterval: (query) => (query.state.data?.state === "provisioning" ? 2000 : false),
  });
}

/** What `POST /datasets` answers: the row as created plus the job doing the
 * slow half. 202, not 201 — the dataset is not usable yet. */
export interface DatasetCreated {
  name: string;
  state: string;
  job_id: string;
}

/**
 * Create a dataset.
 *
 * Invalidates the LIST but not any dataset's detail: the new row is
 * `provisioning`, and the wizard watches it through [`useDataset`], whose own
 * poll stops when it turns active.
 */
export function useCreateDataset(org: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: { name: string; display_name?: string; entity_type: EntityType }) =>
      bffFetch<DatasetCreated>(`/api/orgs/${org}/datasets`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
      }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.datasets(org) });
    },
  });
}

/** The two entity types a dataset can resolve. Fixed at creation: publish and
 * model activation both refuse a document that disagrees with it. */
export type EntityType = DatasetSummary["entity_type"];

/**
 * Suspend or resume one dataset.
 *
 * Both the list and that dataset's detail are invalidated, because the
 * switcher, the manage table and the `[ds]` guard all read the state and a
 * stale one of them would offer a frozen dataset as working.
 */
export function useDatasetLifecycle(org: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ ds, action }: { ds: string; action: "suspend" | "resume" }) =>
      bffFetch<unknown>(`/api/orgs/${org}/datasets/${ds}/${action}`, { method: "POST" }),
    onSettled: (_data, _error, variables) => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.datasets(org) });
      void queryClient.invalidateQueries({ queryKey: queryKeys.dataset(org, variables.ds) });
    },
  });
}

/** Rename a dataset's label; `null` clears it back to the routing name. */
export function useRenameDataset(org: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ ds, displayName }: { ds: string; displayName: string | null }) =>
      bffFetch<unknown>(`/api/orgs/${org}/datasets/${ds}`, {
        method: "PATCH",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ display_name: displayName }),
      }),
    onSettled: (_data, _error, variables) => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.datasets(org) });
      void queryClient.invalidateQueries({ queryKey: queryKeys.dataset(org, variables.ds) });
    },
  });
}
