"use client";

import { useQuery } from "@tanstack/react-query";

import { bffFetch } from "@/lib/api/client";
import { queryKeys } from "./keys";

export interface UsageRow {
  kind: string;
  unit: string;
  units: number;
  events: number;
}

export interface UsageResult {
  days: number;
  since: string;
  rows: UsageRow[];
}

export function useUsage(org: string, days = 30) {
  return useQuery({
    queryKey: queryKeys.usage(org, days),
    queryFn: () => bffFetch<UsageResult>(`/api/orgs/${org}/usage?days=${String(days)}`),
  });
}

/** `ai_assistant` -> "Assistant". The wire name is a key, not a label. */
export function usageLabel(kind: string): string {
  const named: Record<string, string> = {
    ai_assistant: "Assistant",
    ai_enrich: "Record enrichment",
    ai_adjudicate: "Review adjudication",
    ai_config_author: "Config authoring",
    ai_config_explain: "Rule explanations",
    ai_schema_map: "Schema mapping",
  };
  return named[kind] ?? kind;
}
