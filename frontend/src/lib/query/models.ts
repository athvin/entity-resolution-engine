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

/**
 * A confusion matrix and the ratios it implies. **Every ratio is nullable, and
 * null does not mean zero**: a null `precision` is "made no positive
 * predictions on this label set", a precision of 0 is "got every positive
 * wrong". Rendering them the same way would invert what a steward concludes.
 */
export interface ConfusionMetrics {
  tp: number;
  fp: number;
  fn: number;
  tn: number;
  precision: number | null;
  recall: number | null;
  f1: number | null;
  accuracy: number | null;
}

/** Why labels were left out, so a figure says what it was measured on. */
export interface EvaluationCounts {
  labels_for_version: number;
  evaluable: number;
  /** Labels recorded with no score in front of the labeller. */
  no_score: number;
  /** Verdicts outside match/no_match (a dismissal is not a verdict). */
  not_a_verdict: number;
}

export interface ModelEvaluation {
  name: string;
  version: number;
  state: string;
  /** The label the scoring pass stamps on `match_scores`, e.g. `v0001`. */
  model_version_label: string;
  threshold: number;
  counts: EvaluationCounts;
  overall: ConfusionMetrics;
  /** Absent origins are omitted, never present-and-null. */
  by_origin: Partial<Record<"human" | "reviewer" | "llm", ConfusionMetrics>>;
  curve: (ConfusionMetrics & { threshold: number })[];
}

/**
 * How a version scored against the labels collected under it.
 *
 * Only labels carrying that version's own score are evaluable, which is why
 * `counts` ships beside the metrics rather than being inferred from them.
 */
export function useModelEvaluation(org: string, name: string, version: number | null) {
  return useQuery({
    queryKey: queryKeys.modelEvaluation(org, name, version ?? 0),
    queryFn: () =>
      bffFetch<ModelEvaluation>(
        `/api/orgs/${org}/models/${encodeURIComponent(name)}/versions/${String(
          version,
        )}/evaluation`,
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
