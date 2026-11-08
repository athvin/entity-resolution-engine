import type { EntityDetail, GoldenRecord } from "@/lib/query/hooks";

/** One side of a manual merge: the selected golden record plus its members. */
export interface ManualMergeSide {
  record: GoldenRecord;
  detail: EntityDetail;
}

/**
 * Assemble the `POST /merges` bundle for a manual group merge (spec 2.10):
 * the union of every side's member record keys, mastered on the survivor's
 * first member — the same convention the entity-level dialog uses. No
 * `review_id`: its absence is what marks the merge manual in the audit trail
 * (the server derives the origin, the client never declares it).
 *
 * Returns the refusal as a string rather than throwing: the dialog shows it
 * verbatim.
 */
export function manualMergePayload(
  sides: ManualMergeSide[],
  survivorId: string,
): { records: string[]; master: string; apply_now: true } | { error: string } {
  const survivor = sides.find((side) => side.record.id === survivorId);
  if (!survivor) {
    return { error: "Choose which record survives." };
  }
  const master = survivor.detail.members[0]?.record_key;
  if (master === undefined) {
    return { error: "The surviving record has no member records yet." };
  }
  const records = [
    ...new Set(sides.flatMap((side) => side.detail.members.map((m) => m.record_key))),
  ];
  if (records.length < 2) {
    return { error: "Nothing to merge — the selection holds one record." };
  }
  if (records.length > 50) {
    return { error: `Too many records (${String(records.length)}) — a merge takes at most 50.` };
  }
  return { records, master, apply_now: true };
}
