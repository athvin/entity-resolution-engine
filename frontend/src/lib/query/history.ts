"use client";

import { useQueries } from "@tanstack/react-query";

import { bffFetch } from "@/lib/api/client";
import { historyPoints, valueChanged, type RunLike } from "@/lib/history/snapshots";
import type { FieldHistoryRow } from "@/components/records/field-history";

interface DetailAtSnapshot {
  golden: Record<string, unknown>;
}

/**
 * One golden field's value at each offered snapshot (O12c).
 *
 * `useQueries` rather than a loop of `useQuery`: the number of snapshots is
 * data-driven, and hooks cannot be called in a loop whose length changes
 * between renders. Each read is its own cache entry keyed by snapshot, so
 * opening the history for a second field reuses every fetch the first made.
 *
 * `enabled` gates the whole set on the panel being open — a per-field history
 * that fetched eagerly would issue ten reads per entity view for a panel
 * almost nobody opens.
 */
export function useFieldHistory(
  org: string,
  entityId: string,
  attribute: string | null,
  runs: readonly RunLike[],
): { rows: FieldHistoryRow[]; pending: boolean } {
  const points = historyPoints(runs);
  const results = useQueries({
    queries: points.map((point) => ({
      queryKey: ["org", org, "entity-at", entityId, point.snapshot] as const,
      queryFn: () =>
        bffFetch<DetailAtSnapshot>(
          `/api/orgs/${org}/golden-records/${encodeURIComponent(entityId)}?snapshot=${String(point.snapshot)}`,
        ),
      enabled: attribute !== null,
      // The past does not change: a snapshot read is immutable, so refetching
      // it is pure cost.
      staleTime: Number.POSITIVE_INFINITY,
    })),
  });

  if (attribute === null) return { rows: [], pending: false };

  const pending = results.some((result) => result.isPending);
  const values = results.map((result) => {
    const raw = result.data?.golden[attribute];
    return typeof raw === "string" || typeof raw === "number" ? String(raw) : null;
  });

  // `changed` compares each snapshot against the one NEWER than it (index-1),
  // because the list is newest-first: the question a reader has is "when did it
  // become what it is now", which is answered looking backwards.
  const rows: FieldHistoryRow[] = points.map((point, index) => ({
    ...point,
    value: values[index] ?? null,
    changed: index === 0 ? false : valueChanged(values[index], values[index - 1]),
  }));
  return { rows, pending };
}
