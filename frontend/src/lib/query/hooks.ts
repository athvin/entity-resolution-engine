"use client";

import { useQuery } from "@tanstack/react-query";

import { bffFetch } from "@/lib/api/client";
import { queryKeys } from "./keys";

export interface Metrics {
  records: number;
  entities: number;
  duplicate_groups: number;
  records_in_duplicate_groups: number;
  snapshot?: number;
}

export interface RunRow {
  run_id: string;
  mode?: string;
  status?: string;
  started_at?: string;
  finished_at?: string;
  [key: string]: unknown;
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

export function useOpenReviewCount(org: string) {
  return useQuery({
    queryKey: queryKeys.reviews(org),
    queryFn: () => bffFetch<unknown[]>(`/api/orgs/${org}/reviews?limit=500`),
    refetchInterval: 60_000,
    select: (rows) => rows.length,
  });
}
