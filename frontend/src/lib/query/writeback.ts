"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { bffFetch } from "@/lib/api/client";
import { queryKeys } from "./keys";

/** One merge preview as the list shows it (#993/#997). */
export interface MergePreviewSummary {
  id: string;
  connection_id: string;
  object: string;
  entity_id: string;
  fingerprint: string;
  /** `planned`, `approved`, `rejected` or `superseded`. */
  status: string;
  master_record_key: string;
  member_count: number;
  /** Weakest-link match evidence, 0–1; null when absent. */
  score: number | null;
  /** A principal, or `policy:<rule id>` for an auto-approval. */
  decided_by: string | null;
  lake_snapshot: number;
  policy_revision: string;
  created_at: string;
}

export interface MergePreviewPage {
  items: MergePreviewSummary[];
  next_cursor: string | null;
}

export interface MemberSnapshot {
  record_key: string;
  external_id: string;
  system_modstamp: string;
  fields: Record<string, unknown>;
}

export interface MergePreviewDetail {
  summary: MergePreviewSummary;
  proposal: {
    master_record_key?: string;
    members?: MemberSnapshot[];
  };
  field_changes: Record<string, { before: unknown; after: unknown }>;
  related_counts: Record<string, unknown> | null;
  related_counts_state: string;
  decided_by: string | null;
  decision_reason: string | null;
  history: { from_status: string | null; to_status: string; actor: string; at: string }[];
  apply: { status: string; detail: string | null } | null;
}

export interface RestoreView {
  status: string;
  conflicts: { record?: string; field?: string; reason: string }[];
  detail: string | null;
  steps: {
    kind: string;
    target_id: string;
    status: string;
    outcome: string | null;
    new_id: string | null;
  }[];
  related_records_reverted: boolean;
  job_id: string | null;
}

export interface BatchResult {
  results: { id: string; status: number; preview_status: string | null; detail: string | null }[];
}

export interface AutoApprovalRule {
  id: string;
  connection_id: string;
  object: string;
  enabled: boolean;
  min_score: number;
  max_members: number;
  created_by: string;
  created_at: string;
}

const base = (org: string, ds: string) => `/api/orgs/${org}/datasets/${ds}`;
const root = (org: string, ds: string) => [...queryKeys.ds(org, ds), "writeback"] as const;

export function useMergePreviews(org: string, ds: string, status: string) {
  return useQuery({
    queryKey: [...root(org, ds), "previews", status],
    queryFn: () => {
      const params = new URLSearchParams({ limit: "100" });
      if (status) params.set("status", status);
      return bffFetch<MergePreviewPage>(`${base(org, ds)}/merge-previews?${params.toString()}`);
    },
  });
}

export function useMergePreview(org: string, ds: string, id: string | null) {
  return useQuery({
    queryKey: [...root(org, ds), "preview", id],
    queryFn: () =>
      bffFetch<MergePreviewDetail>(
        `${base(org, ds)}/merge-previews/${encodeURIComponent(id ?? "")}`,
      ),
    enabled: id !== null,
  });
}

export function useRestore(org: string, ds: string, id: string | null, enabled: boolean) {
  return useQuery({
    queryKey: [...root(org, ds), "restore", id],
    queryFn: () =>
      bffFetch<RestoreView>(
        `${base(org, ds)}/merge-previews/${encodeURIComponent(id ?? "")}/restore`,
      ),
    enabled: enabled && id !== null,
    retry: false,
  });
}

export function useAutoApproval(org: string, ds: string) {
  return useQuery({
    queryKey: [...root(org, ds), "auto-approval"],
    queryFn: () =>
      bffFetch<{ items: AutoApprovalRule[] }>(`${base(org, ds)}/writeback/auto-approval`),
  });
}

function post<T>(url: string, body?: unknown, method = "POST"): Promise<T> {
  return bffFetch<T>(url, {
    method,
    headers: { "content-type": "application/json" },
    body: body === undefined ? null : JSON.stringify(body),
  });
}

/** Every writeback mutation invalidates the whole writeback surface: a
 * decision, an apply or an undo each move several previews' states. */
function useWritebackMutation<V, T>(org: string, ds: string, fn: (vars: V) => Promise<T>) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: fn,
    onSettled: () => queryClient.invalidateQueries({ queryKey: root(org, ds) }),
  });
}

export function useDecideBatch(org: string, ds: string) {
  return useWritebackMutation(
    org,
    ds,
    (vars: { decision: "approve" | "reject"; items: { id: string; fingerprint: string }[] }) =>
      post<BatchResult>(`${base(org, ds)}/merge-previews/decide`, vars),
  );
}

export function useGeneratePreviews(org: string, ds: string) {
  return useWritebackMutation(org, ds, () =>
    post<{ job_id: string; state: string }>(`${base(org, ds)}/merge-previews/generate`),
  );
}

export function useApplyApproved(org: string, ds: string) {
  return useWritebackMutation(org, ds, () =>
    post<{ job_id: string; state: string }>(`${base(org, ds)}/merge-previews/apply`),
  );
}

export function useRequestRestore(org: string, ds: string) {
  return useWritebackMutation(org, ds, (id: string) =>
    post<RestoreView>(`${base(org, ds)}/merge-previews/${encodeURIComponent(id)}/restore`),
  );
}

export function useSetAutoApproval(org: string, ds: string) {
  return useWritebackMutation(
    org,
    ds,
    (rule: Omit<AutoApprovalRule, "id" | "created_by" | "created_at">) =>
      post<AutoApprovalRule>(`${base(org, ds)}/writeback/auto-approval`, rule, "PUT"),
  );
}

/** A proposed Salesforce upsert (#998). */
export interface UpsertPreview {
  id: string;
  connection_id: string;
  object: string;
  entity_id: string;
  external_id_field: string;
  external_id_value: string;
  fields: Record<string, unknown>;
  fingerprint: string;
  /** `planned`, `approved`, `rejected` or `superseded`. */
  status: string;
  decided_by: string | null;
  policy_revision: string;
  /** `applying`, `created`, `updated`, `failed` or `skipped_stale`, once begun. */
  apply_status: string | null;
  sf_record_id: string | null;
  apply_detail: string | null;
  created_at: string;
}

export function useUpsertPreviews(org: string, ds: string) {
  return useQuery({
    queryKey: [...root(org, ds), "upserts"],
    queryFn: () =>
      bffFetch<{ items: UpsertPreview[]; next_cursor: string | null }>(
        `${base(org, ds)}/upsert-previews?limit=100`,
      ),
  });
}

export function useDecideUpserts(org: string, ds: string) {
  return useWritebackMutation(
    org,
    ds,
    (vars: { decision: "approve" | "reject"; items: { id: string; fingerprint: string }[] }) =>
      post<BatchResult>(`${base(org, ds)}/upsert-previews/decide`, vars),
  );
}

export function useApplyUpserts(org: string, ds: string) {
  return useWritebackMutation(org, ds, () =>
    post<{ job_id: string; state: string }>(`${base(org, ds)}/upsert-previews/apply`),
  );
}

/** A proposed Lead → Contact conversion (#520). Not reversible in Salesforce. */
export interface LeadConversion {
  id: string;
  connection_id: string;
  entity_id: string;
  lead_external_id: string;
  lead_record_key: string;
  contact_external_id: string;
  contact_record_key: string;
  fingerprint: string;
  /** `planned`, `approved`, `rejected` or `superseded`. */
  status: string;
  decided_by: string | null;
  /** `applying`, `converted`, `converted_externally`, `failed` or `skipped_stale`, once begun. */
  apply_status: string | null;
  account_id: string | null;
  apply_detail: string | null;
  created_at: string;
}

export function useLeadConversions(org: string, ds: string) {
  return useQuery({
    queryKey: [...root(org, ds), "conversions"],
    queryFn: () =>
      bffFetch<{ items: LeadConversion[]; next_cursor: string | null }>(
        `${base(org, ds)}/lead-conversions?limit=100`,
      ),
  });
}

export function useDecideConversions(org: string, ds: string) {
  return useWritebackMutation(
    org,
    ds,
    (vars: { decision: "approve" | "reject"; items: { id: string; fingerprint: string }[] }) =>
      post<BatchResult>(`${base(org, ds)}/lead-conversions/decide`, vars),
  );
}

export function useApplyConversions(org: string, ds: string) {
  return useWritebackMutation(org, ds, () =>
    post<{ job_id: string; state: string }>(`${base(org, ds)}/lead-conversions/apply`),
  );
}
