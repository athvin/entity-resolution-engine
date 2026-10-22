"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { bffFetch } from "@/lib/api/client";
import { queryKeys } from "./keys";

/** One version as the index lists it — no settings document (it is large). */
export interface ModelVersionRow {
  id: string;
  name: string;
  version: number;
  entity_type: string;
  model_hash: string;
  model_version_label: string;
  tf_snapshot_id: string | null;
  state: "draft" | "active" | "archived";
  provenance: { origin?: string; by?: string } | null;
  created_by: string;
  activated_at: string | null;
  created_at: string;
}

/**
 * `active` is a LIST, one entry per name: activation is unique per
 * `(org, name)`, so a tenant running an ensemble has several active at once.
 */
export interface ModelIndex {
  items: ModelVersionRow[];
  active: { name: string; version: number; model_hash: string }[];
}

/**
 * One comparison level's learned probabilities and the weight they imply.
 *
 * Field names are the server's, read off `model_weights` rather than guessed:
 * the weight is `log2_bayes_factor`, and it is null for a level where either
 * probability is missing (a null level) or non-positive.
 */
export interface WeightLevel {
  label: string;
  sql_condition: string;
  is_null_level: boolean;
  m_probability: number | null;
  u_probability: number | null;
  log2_bayes_factor: number | null;
  tf_adjustment_column: string | null;
}

export interface WeightComparison {
  output_column_name: string;
  comparison_description: string | null;
  levels: WeightLevel[];
}

export interface ModelWeights {
  link_type: string;
  /** The prior: P(two random records match). */
  probability_two_random_records_match: number | null;
  blocking_rules: unknown[];
  comparisons: WeightComparison[];
}

export interface ModelVersionDetail extends ModelVersionRow {
  model_json: string;
  /** Null when the stored document cannot be parsed as settings. */
  weights: ModelWeights | null;
}

export function useModels(org: string) {
  return useQuery({
    queryKey: queryKeys.models(org),
    queryFn: () => bffFetch<ModelIndex>(`/api/orgs/${org}/models`),
  });
}

export function useModelVersion(org: string, name: string, version: number | null) {
  return useQuery({
    queryKey: queryKeys.modelVersion(org, name, version ?? 0),
    queryFn: () =>
      bffFetch<ModelVersionDetail>(
        `/api/orgs/${org}/models/${encodeURIComponent(name)}/versions/${String(version)}`,
      ),
    enabled: version !== null && name !== "",
  });
}

/** Everything that changes the registry invalidates the same index. */
function useRegistryMutation<T>(org: string, run: () => Promise<T>) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: run,
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.models(org) });
    },
  });
}

export function useActivateModel(org: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ name, version }: { name: string; version: number }) =>
      bffFetch<unknown>(
        `/api/orgs/${org}/models/${encodeURIComponent(name)}/versions/${String(version)}/activate`,
        { method: "POST" },
      ),
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.models(org) });
    },
  });
}

/** Compile a draft from the org's active published config. No body. */
export function useCompileModel(org: string) {
  return useRegistryMutation(org, () =>
    bffFetch<ModelVersionRow>(`/api/orgs/${org}/models/compile`, { method: "POST" }),
  );
}

export interface ModelUpload {
  model_json: string;
  model_version_label: string;
  name?: string;
  tf_snapshot_id?: string;
}

export function useUploadModel(org: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: ModelUpload) =>
      bffFetch<ModelVersionRow>(`/api/orgs/${org}/models/versions`, {
        method: "POST",
        body: JSON.stringify(body),
      }),
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.models(org) });
    },
  });
}
