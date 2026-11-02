"use client";

import { useQuery } from "@tanstack/react-query";

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

/** The org's datasets, for the switcher. */
export function useDatasets(org: string) {
  return useQuery({
    queryKey: queryKeys.datasets(org),
    queryFn: () => bffFetch<DatasetSummary[]>(`/api/orgs/${org}/datasets`),
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
