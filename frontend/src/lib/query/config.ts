"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { bffFetch } from "@/lib/api/client";
import { parseConfig } from "@/lib/domain/config";
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

/** The whole config family: active, the version list, and each version.
 * Exported so `config-ai.ts` invalidates the SAME key rather than a parallel
 * one that would leave the version history card stale after an AI draft. */
export const configKey = (org: string, ds: string) => ["org", org, "ds", ds, "config"] as const;

export function useActiveConfig(org: string, ds: string) {
  return useQuery({
    queryKey: [...configKey(org, ds), "active"],
    queryFn: () => bffFetch<ConfigVersionDetail>(`/api/orgs/${org}/datasets/${ds}/config`),
  });
}

/**
 * The org's entity type, read from its active config. "person" until the config
 * loads, if it omits `entity_type`, or on a parse error — so person tenants and
 * the loading state render exactly as before.
 */
export function useOrgEntityType(org: string, ds: string): "person" | "account" {
  const { data } = useActiveConfig(org, ds);
  if (!data?.yaml) return "person";
  try {
    return parseConfig(data.yaml).entityType;
  } catch {
    return "person";
  }
}

export function useConfigVersions(org: string, ds: string) {
  return useQuery({
    queryKey: [...configKey(org, ds), "versions"],
    queryFn: () => bffFetch<ConfigVersionRow[]>(`/api/orgs/${org}/datasets/${ds}/config/versions`),
  });
}

export function useConfigVersion(org: string, ds: string, version: number | null) {
  return useQuery({
    queryKey: [...configKey(org, ds), "version", version],
    queryFn: () =>
      bffFetch<ConfigVersionDetail>(
        `/api/orgs/${org}/datasets/${ds}/config/versions/${String(version)}`,
      ),
    enabled: version !== null,
  });
}

export function useCreateDraft(org: string, ds: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (yaml: string) =>
      bffFetch<{ version: number }>(`/api/orgs/${org}/datasets/${ds}/config/versions`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ yaml }),
      }),
    onSettled: () => void queryClient.invalidateQueries({ queryKey: configKey(org, ds) }),
  });
}

export function usePublish(org: string, ds: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (version: number) =>
      bffFetch<PublishResult>(
        `/api/orgs/${org}/datasets/${ds}/config/versions/${String(version)}/publish`,
        {
          method: "POST",
        },
      ),
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: configKey(org, ds) });
      void queryClient.invalidateQueries({ queryKey: queryKeys.jobs(org, ds) });
      void queryClient.invalidateQueries({ queryKey: [...queryKeys.org(org), "sources"] });
    },
  });
}

interface ScoreRow {
  match_probability: number;
}

/** A sample of recent scored pairs + open reviews — the histogram's input. */
export function useScoreSample(org: string, ds: string) {
  return useQuery({
    queryKey: [...queryKeys.org(org), "score-sample"],
    queryFn: async () => {
      const [scores, reviews] = await Promise.all([
        bffFetch<{ items: ScoreRow[] }>(`/api/orgs/${org}/datasets/${ds}/match-scores?limit=200`),
        bffFetch<{ items: { match_probability: number | null }[] }>(
          `/api/orgs/${org}/datasets/${ds}/reviews?limit=200`,
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
