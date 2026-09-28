"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ulid } from "ulidx";

import { bffFetch } from "@/lib/api/client";
import { queryKeys } from "./keys";

export interface ScheduleRow {
  schedule_id: string;
  kind: string;
  cron: string;
  enabled: boolean;
  source: string;
  last_enqueued_at: string | null;
}

export interface SourceInfo {
  name: string;
  priority_rank: number | null;
  drop_subdir: string | null;
}

export interface SourcesPayload {
  config_version: number;
  sources: SourceInfo[];
}

export function useSchedules(org: string) {
  return useQuery({
    queryKey: [...queryKeys.org(org), "schedules"],
    queryFn: () => bffFetch<ScheduleRow[]>(`/api/orgs/${org}/schedules`),
  });
}

export function useCreateSchedule(org: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: { kind: string; cron: string }) =>
      bffFetch<unknown>(`/api/orgs/${org}/schedules`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
      }),
    onSettled: () =>
      void queryClient.invalidateQueries({ queryKey: [...queryKeys.org(org), "schedules"] }),
  });
}

export function useDeleteSchedule(org: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (scheduleId: string) =>
      bffFetch<unknown>(`/api/orgs/${org}/schedules/${scheduleId}`, { method: "DELETE" }),
    onSettled: () =>
      void queryClient.invalidateQueries({ queryKey: [...queryKeys.org(org), "schedules"] }),
  });
}

export function useToggleSchedule(org: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ scheduleId, enabled }: { scheduleId: string; enabled: boolean }) =>
      bffFetch<unknown>(`/api/orgs/${org}/schedules/${scheduleId}`, {
        method: "PATCH",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ enabled }),
      }),
    // Control-plane write: optimistic flip, rollback on failure.
    onMutate: async ({ scheduleId, enabled }) => {
      const key = [...queryKeys.org(org), "schedules"];
      await queryClient.cancelQueries({ queryKey: key });
      const previous = queryClient.getQueryData<ScheduleRow[]>(key);
      queryClient.setQueryData<ScheduleRow[]>(key, (rows) =>
        (rows ?? []).map((row) => (row.schedule_id === scheduleId ? { ...row, enabled } : row)),
      );
      return { previous };
    },
    onError: (_error, _vars, context) => {
      if (context?.previous) {
        queryClient.setQueryData([...queryKeys.org(org), "schedules"], context.previous);
      }
    },
    onSettled: () =>
      void queryClient.invalidateQueries({ queryKey: [...queryKeys.org(org), "schedules"] }),
  });
}

export function useSources(org: string) {
  return useQuery({
    queryKey: [...queryKeys.org(org), "sources"],
    queryFn: () => bffFetch<SourcesPayload>(`/api/orgs/${org}/sources`),
  });
}

export interface ImportResult {
  job: { job_id: string };
  batch_id?: string;
}

export function useImportFile(org: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async ({ source, file }: { source: string; file: File }) => {
      const body = new FormData();
      body.set("file", file);
      return bffFetch<ImportResult>(
        `/api/orgs/${org}/imports?source=${encodeURIComponent(source)}`,
        { method: "POST", body },
      );
    },
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.jobs(org) });
    },
  });
}

export function useSubmitJob(org: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: { kind: string; params?: Record<string, unknown> }) =>
      bffFetch<{ job_id: string }>(`/api/orgs/${org}/jobs/submit`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ ...body, intent: ulid().toLowerCase() }),
      }),
    onSettled: () => void queryClient.invalidateQueries({ queryKey: queryKeys.jobs(org) }),
  });
}
