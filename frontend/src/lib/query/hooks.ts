"use client";

import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { bffFetch } from "@/lib/api/client";
import { queryKeys } from "./keys";

export interface Metrics {
  records: number;
  entities: number;
  duplicate_groups: number;
  records_in_duplicate_groups: number;
  open_reviews: number;
  snapshot?: number;
}

export interface RunRow {
  run_id: string;
  mode?: string;
  status?: string;
  started_at?: string;
  ended_at?: string;
  config_hash?: string;
  model_version?: string;
  rebuild_reason?: string;
  [key: string]: unknown;
}

/** One golden record as the read API serves it (the nine canonical attributes+). */
export interface GoldenRecord {
  entity_id: string;
  given_name: string | null;
  family_name: string | null;
  email: string | null;
  phone_e164: string | null;
  addr_number: string | null;
  addr_street: string | null;
  addr_unit: string | null;
  addr_city: string | null;
  addr_region: string | null;
  addr_postal: string | null;
  birth_date: string | null;
  survivorship_version?: number;
  assembled_at?: string;
}

export interface GoldenPage {
  items: GoldenRecord[];
  snapshot: number;
  next_cursor: string | null;
}

export interface EntityMember {
  source_system: string;
  source_record_id: string;
  record_key: string;
  assigned_at?: string;
  run_id?: string;
}

export interface LineageRow {
  attribute: string;
  record_key: string;
  source_system: string;
  source_record_id: string;
  rule: string;
}

export interface EntityEvent {
  seq: number;
  run_id: string;
  event_type: string;
  details: unknown;
  occurred_at: string;
}

export interface EntityDetail {
  golden: GoldenRecord;
  members: EntityMember[];
  lineage: LineageRow[];
  events: EntityEvent[];
}

export interface JobRow {
  job_id: string;
  kind: string;
  state: string;
  error_class: string | null;
  attempt: number;
  progress: { stages?: { stage: string }[] };
}

const ACTIVE_JOB_STATES = new Set(["queued", "dispatching", "running", "retrying", "canceling"]);

export interface SessionInfo {
  user: { id: string; email: string; displayName: string; isSuperAdmin: boolean };
  memberships: { org: string; role: "viewer" | "steward" | "admin" }[];
  impersonating: { org: string; role: "viewer" | "steward" | "admin"; expiresAt: string } | null;
}

export function useSession() {
  return useQuery({
    queryKey: queryKeys.session(),
    queryFn: () => bffFetch<SessionInfo>("/api/auth/session"),
    staleTime: 60_000,
  });
}

/** The effective role inside `org` — mirrors the BFF's requireOrgAccess exactly. */
export function effectiveRole(
  session: SessionInfo | undefined,
  org: string,
): "viewer" | "steward" | "admin" | null {
  if (!session) return null;
  if (session.impersonating && session.user.isSuperAdmin) {
    return session.impersonating.org === org ? session.impersonating.role : null;
  }
  const membership = session.memberships.find((m) => m.org === org);
  if (membership) return membership.role;
  return session.user.isSuperAdmin ? "admin" : null;
}

export function useMetrics(org: string) {
  return useQuery({
    queryKey: queryKeys.metrics(org),
    queryFn: () => bffFetch<Metrics>(`/api/orgs/${org}/metrics`),
    refetchInterval: 30_000,
  });
}

export function useRuns(org: string) {
  return useQuery({
    queryKey: queryKeys.runs(org),
    queryFn: () => bffFetch<RunRow[]>(`/api/orgs/${org}/runs`),
    refetchInterval: 30_000,
  });
}

export function useJobs(org: string) {
  return useQuery({
    queryKey: queryKeys.jobs(org),
    queryFn: () => bffFetch<JobRow[]>(`/api/orgs/${org}/jobs?limit=20`),
    // Poll fast while anything is in flight; settle down when quiet.
    refetchInterval: (query) =>
      (query.state.data ?? []).some((job) => ACTIVE_JOB_STATES.has(job.state)) ? 1_500 : 30_000,
  });
}

export function useGoldenRecords(org: string, q: string) {
  return useInfiniteQuery({
    queryKey: queryKeys.goldenRecords(org, q),
    queryFn: ({ pageParam }) => {
      const params = new URLSearchParams({ limit: "100" });
      if (q) params.set("q", q);
      if (pageParam) params.set("cursor", pageParam);
      return bffFetch<GoldenPage>(`/api/orgs/${org}/golden-records?${params.toString()}`);
    },
    initialPageParam: "",
    getNextPageParam: (last) => last.next_cursor ?? undefined,
  });
}

export function useEntityDetail(org: string, entityId: string) {
  return useQuery({
    queryKey: queryKeys.entity(org, entityId),
    queryFn: () => bffFetch<EntityDetail>(`/api/orgs/${org}/golden-records/${entityId}`),
  });
}

export function useJob(org: string, jobId: string) {
  return useQuery({
    queryKey: queryKeys.job(org, jobId),
    queryFn: () => bffFetch<JobRow>(`/api/orgs/${org}/jobs/${jobId}`),
    refetchInterval: (query) =>
      query.state.data && ACTIVE_JOB_STATES.has(query.state.data.state) ? 1_500 : false,
  });
}

export function useJobAction(org: string, jobId: string, action: "cancel" | "resume") {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () =>
      bffFetch<unknown>(`/api/orgs/${org}/jobs/${jobId}/${action}`, { method: "POST" }),
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.job(org, jobId) });
      void queryClient.invalidateQueries({ queryKey: queryKeys.jobs(org) });
    },
  });
}
