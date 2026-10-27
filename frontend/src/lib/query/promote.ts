"use client";

import { useMutation, useQueryClient } from "@tanstack/react-query";

import { bffFetch } from "@/lib/api/client";
import type { DiffLine } from "@/lib/domain/config";
import { configKey } from "./config";

export interface CompareResult {
  org: string;
  against: string;
  diff: DiffLine[];
  their_version: number | null;
  my_version: number | null;
}

/** Compare this org's published config against another's. */
export function useCompareConfigs(org: string) {
  return useMutation({
    mutationFn: (input: { against: string; version?: number }) =>
      bffFetch<CompareResult>(`/api/orgs/${org}/config/promote`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(input),
      }),
  });
}

export interface ImportResult {
  imported_from: string;
  imported_version: number | null;
  version: unknown;
}

/**
 * Copy another org's config in as a draft.
 *
 * Invalidates the config family so the new draft appears in the version
 * history — which is the review surface, and the only place someone will see
 * what they just staged.
 */
export function useImportConfig(org: string) {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (input: { from: string; version?: number }) =>
      bffFetch<ImportResult>(`/api/orgs/${org}/config/promote`, {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(input),
      }),
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: configKey(org) });
    },
  });
}

/**
 * The orgs a promotion may target.
 *
 * Only those the person ADMINISTERS, and never the current one. The server
 * checks both sides independently — this list is a convenience, not the
 * control — but offering an org the caller cannot read would produce a 404
 * they have no way to interpret.
 */
export function promotableOrgs(
  memberships: readonly { org: string; role: string }[],
  current: string,
): string[] {
  return memberships
    .filter((membership) => membership.role === "admin" && membership.org !== current)
    .map((membership) => membership.org)
    .sort();
}
