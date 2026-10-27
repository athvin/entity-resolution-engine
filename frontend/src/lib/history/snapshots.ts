/**
 * Which points in history a field's value can be read at (O12c).
 *
 * **No new table.** DuckLake keeps every version of `golden_records` and
 * `golden_lineage`, and the run ledger already records the snapshot range each
 * run produced. So "what did this field used to be" is answered by reading the
 * entity at an older lake version — the history exists, it was simply never
 * reachable.
 *
 * The alternative would have been a change-log table written on every golden
 * assembly, which is a second source of truth for something the lake already
 * stores exactly.
 */

export interface RunLike {
  id: string;
  status?: string;
  ended_at?: string;
  [key: string]: unknown;
}

export interface HistoryPoint {
  /** The lake version to read the entity at. */
  snapshot: number;
  /** The run that produced it, for the label. */
  runId: string;
  endedAt: string | null;
}

/** Read a numeric field that may arrive as a string or be absent. */
function numeric(value: unknown): number | null {
  if (typeof value === "number" && Number.isInteger(value)) return value;
  if (typeof value === "string" && /^\d+$/.test(value)) return Number(value);
  return null;
}

/**
 * The snapshots worth offering, newest first.
 *
 * Only COMPLETED runs: a failed or running one may have produced a partial
 * snapshot range, and offering a reader the entity "as of" a half-finished run
 * would show them a state the product never actually published.
 *
 * De-duplicated by snapshot, because several runs can end on the same version
 * when one produced no changes — two identical entries in a history picker is
 * a choice with no difference.
 */
export function historyPoints(runs: readonly RunLike[], limit = 10): HistoryPoint[] {
  const seen = new Set<number>();
  const points: HistoryPoint[] = [];
  for (const run of runs) {
    if (run.status !== "completed") continue;
    const snapshot = numeric(run.snapshot_end);
    if (snapshot === null || seen.has(snapshot)) continue;
    seen.add(snapshot);
    points.push({
      snapshot,
      runId: run.id,
      endedAt: typeof run.ended_at === "string" ? run.ended_at : null,
    });
  }
  return points.sort((a, b) => b.snapshot - a.snapshot).slice(0, limit);
}

/**
 * Did this attribute's value change between two reads?
 *
 * Absence and empty string are the same thing — a field that was never set and
 * one set to "" are indistinguishable to a reader, and treating them as a
 * change would fill the history with differences nobody made.
 */
export function valueChanged(before: unknown, after: unknown): boolean {
  const normalise = (value: unknown) =>
    typeof value === "string" && value.trim() !== "" ? value.trim() : null;
  return normalise(before) !== normalise(after);
}
