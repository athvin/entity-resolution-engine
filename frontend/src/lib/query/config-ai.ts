"use client";

import { useMutation, useQueryClient } from "@tanstack/react-query";

import { bffFetch } from "@/lib/api/client";
import { configKey } from "./config";

export interface DraftFromProseResult {
  /** False when the model declined — an expected answer, not an error. */
  drafted: boolean;
  /** Present when drafted: what it changed. */
  summary?: string;
  /** Present when it declined: why. */
  reason?: string;
  version?: unknown;
}

/**
 * Draft a config change from a description.
 *
 * Invalidates the version list on success, because the new draft has to appear
 * in the history card — that card IS the review surface, and a draft nobody can
 * see is a draft nobody will read before publishing something else.
 */
export function useDraftFromProse(org: string, ds: string) {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (instruction: string) =>
      bffFetch<DraftFromProseResult>(`/api/orgs/${org}/datasets/${ds}/config/draft-from-prose`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ instruction }),
      }),
    onSuccess: (result) => {
      if (result.drafted) {
        void client.invalidateQueries({ queryKey: configKey(org, ds) });
      }
    },
  });
}

export interface ExplanationResult {
  block: string;
  explanation: string;
}

export function useExplainRule(org: string, ds: string) {
  return useMutation({
    mutationFn: (input: { block: string; rule: string }) =>
      bffFetch<ExplanationResult>(`/api/orgs/${org}/datasets/${ds}/config/explain`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(input),
      }),
  });
}

export interface StarterResult {
  summary: string;
  version?: unknown;
}

/**
 * Propose a STARTING matching config (V8b).
 *
 * Invalidates the config family for the reason `useDraftFromProse` does: the
 * new draft has to appear in the version history, which is the review surface.
 */
export function useStarterConfig(org: string, ds: string) {
  const client = useQueryClient();
  return useMutation({
    mutationFn: () =>
      bffFetch<StarterResult>(`/api/orgs/${org}/datasets/${ds}/config/starter`, { method: "POST" }),
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: configKey(org, ds) });
    },
  });
}
