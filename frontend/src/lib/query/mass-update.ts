"use client";

import { useMutation, useQueryClient } from "@tanstack/react-query";

import { bffFetch } from "@/lib/api/client";

import { type GoldenPage } from "./hooks";
import { queryKeys } from "./keys";

/** The server's per-call ceiling (one writer-lock window). */
export const MASS_UPDATE_CHUNK = 200;
/** The honest ceiling on "edit everything matching this filter". */
export const MASS_UPDATE_CAP = 2_000;

/** The 11 overridable golden fields (mirrors the server's OVERRIDABLE_ATTRS). */
export const GOLDEN_FIELDS = [
  "given_name",
  "family_name",
  "email",
  "phone_e164",
  "birth_date",
  "addr_number",
  "addr_street",
  "addr_unit",
  "addr_city",
  "addr_region",
  "addr_postal",
] as const;
export type GoldenField = (typeof GOLDEN_FIELDS)[number];

export interface MassUpdateItem {
  entity_id: string;
  attribute: GoldenField;
  value?: string | null;
  clear?: boolean;
}

/**
 * Walk the golden-records cursor pages and collect entity ids for the whole
 * filter, up to {@link MASS_UPDATE_CAP}. Returns the ids and whether the cap
 * truncated them — a capped selection must say so rather than read as
 * "everything".
 */
export async function fetchGoldenIdsMatching(
  org: string,
  q: string,
  onProgress?: (count: number) => void,
): Promise<{ ids: string[]; capped: boolean }> {
  const ids: string[] = [];
  let cursor: string | null = null;
  for (;;) {
    const params = new URLSearchParams({ limit: "200" });
    if (q) params.set("q", q);
    if (cursor) params.set("cursor", cursor);
    const page = await bffFetch<GoldenPage>(`/api/orgs/${org}/golden-records?${params.toString()}`);
    for (const item of page.items) {
      if (ids.length >= MASS_UPDATE_CAP) return { ids, capped: true };
      ids.push(item.entity_id);
    }
    onProgress?.(ids.length);
    cursor = page.next_cursor;
    if (cursor === null) return { ids, capped: false };
  }
}

interface MassUpdateResult {
  status: string;
  action_ids: string[];
  apply_job?: string;
}

export function useMassUpdate(org: string) {
  const queryClient = useQueryClient();
  return useMutation({
    // Chunked to the server's 200-per-lock-window cap; each chunk applies now.
    mutationFn: async (items: MassUpdateItem[]) => {
      let last: MassUpdateResult | null = null;
      for (let start = 0; start < items.length; start += MASS_UPDATE_CHUNK) {
        const chunk = items.slice(start, start + MASS_UPDATE_CHUNK);
        last = await bffFetch<MassUpdateResult>(`/api/orgs/${org}/mass-update`, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ items: chunk, apply_now: true }),
        });
      }
      if (last === null) throw new Error("mass update needs at least one item");
      return last;
    },
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: [...queryKeys.org(org), "staged"] });
      void queryClient.invalidateQueries({ queryKey: queryKeys.jobs(org) });
    },
  });
}
