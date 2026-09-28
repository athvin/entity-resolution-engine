"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { bffFetch } from "@/lib/api/client";
import { queryKeys } from "./keys";

export interface ConfigVersionRow {
  version: number;
  config_hash: string;
  state: "draft" | "published";
  tier: string | null;
  created_by: string;
  created_at: string;
  published_at: string | null;
}

export interface ConfigVersionDetail extends ConfigVersionRow {
  yaml: string;
}

export interface PublishResult {
  org: string;
  version: number;
  tier: string | null;
  changed_blocks: string[];
  jobs_enqueued: string[];
}

const configKey = (org: string) => [...queryKeys.org(org), "config"] as const;

export function useActiveConfig(org: string) {
  return useQuery({
    queryKey: [...configKey(org), "active"],
    queryFn: () => bffFetch<ConfigVersionDetail>(`/api/orgs/${org}/config`),
  });
}

export function useConfigVersions(org: string) {
  return useQuery({
    queryKey: [...configKey(org), "versions"],
    queryFn: () => bffFetch<ConfigVersionRow[]>(`/api/orgs/${org}/config/versions`),
  });
}

export function useConfigVersion(org: string, version: number | null) {
  return useQuery({
    queryKey: [...configKey(org), "version", version],
    queryFn: () =>
      bffFetch<ConfigVersionDetail>(`/api/orgs/${org}/config/versions/${String(version)}`),
    enabled: version !== null,
  });
}

export function useCreateDraft(org: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (yaml: string) =>
      bffFetch<{ version: number }>(`/api/orgs/${org}/config/versions`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ yaml }),
      }),
    onSettled: () => void queryClient.invalidateQueries({ queryKey: configKey(org) }),
  });
}

export function usePublish(org: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (version: number) =>
      bffFetch<PublishResult>(`/api/orgs/${org}/config/versions/${String(version)}/publish`, {
        method: "POST",
      }),
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: configKey(org) });
      void queryClient.invalidateQueries({ queryKey: queryKeys.jobs(org) });
      void queryClient.invalidateQueries({ queryKey: [...queryKeys.org(org), "sources"] });
    },
  });
}

interface ScoreRow {
  match_probability: number;
}

/** A sample of recent scored pairs + open reviews — the histogram's input. */
export function useScoreSample(org: string) {
  return useQuery({
    queryKey: [...queryKeys.org(org), "score-sample"],
    queryFn: async () => {
      const [scores, reviews] = await Promise.all([
        bffFetch<{ items: ScoreRow[] }>(`/api/orgs/${org}/match-scores?limit=200`),
        bffFetch<{ items: { match_probability: number | null }[] }>(
          `/api/orgs/${org}/reviews?limit=200`,
        ),
      ]);
      return [
        ...scores.items.map((row) => row.match_probability),
        ...reviews.items.map((row) => row.match_probability).filter((p): p is number => p !== null),
      ];
    },
    staleTime: 60_000,
  });
}
