import "server-only";
/**
 * Which rows are worth exporting (F4).
 *
 * The need behind #86 is plain: do not send a campaign to records you cannot
 * reach. An export of 10,000 rows where 3,000 have no email is not a segment,
 * it is a 30% bounce rate someone discovers later.
 *
 * **This is a COMPLETENESS check on the exported columns, not a read of V3's
 * `record_quality` scores.** Those are real and built — but they are scored
 * per SOURCE RECORD while an export is per ENTITY, so using them would want a
 * declared read joining golden → members → quality. That is a worthwhile
 * follow-up and deliberately not smuggled in here; what this does is answer
 * the question with data the export already has in hand.
 */

/** The ways a row can be "reachable enough" to export. */
export const EXPORT_REQUIREMENTS = ["any", "email", "phone", "postal"] as const;
export type ExportRequirement = (typeof EXPORT_REQUIREMENTS)[number];

/** Which golden column each requirement reads. `any` reads none. */
const COLUMN_FOR: Record<Exclude<ExportRequirement, "any">, string> = {
  email: "email",
  phone: "phone_e164",
  postal: "addr_postal",
};

/**
 * Is this value actually present?
 *
 * Whitespace is absence. A column holding `" "` is indistinguishable from an
 * empty one to anyone receiving the file, and treating it as present is how a
 * "verified" export still bounces.
 */
function present(value: unknown): boolean {
  if (typeof value === "string") return value.trim() !== "";
  return typeof value === "number";
}

/**
 * Should this row be exported under this requirement?
 *
 * `any` exports everything, which is the behaviour every existing caller had
 * before this option existed — the default has to be the old behaviour or
 * adding the option silently changes what every saved export produces.
 */
export function isExportable(
  row: Record<string, unknown>,
  requirement: ExportRequirement,
): boolean {
  if (requirement === "any") return true;
  return present(row[COLUMN_FOR[requirement]]);
}

export function isExportRequirement(value: string): value is ExportRequirement {
  return (EXPORT_REQUIREMENTS as readonly string[]).includes(value);
}

/**
 * How a filtered export describes itself.
 *
 * The count of what was LEFT OUT is the number that matters, and it has to be
 * reported rather than inferred: an export that silently returns 7,000 of
 * 10,000 rows looks like a broken query, and the reader's next move is to
 * re-run it rather than to trust it.
 */
export function exportSummary(
  kept: number,
  skipped: number,
  requirement: ExportRequirement,
): string {
  if (requirement === "any") return `${String(kept)} rows`;
  // "an email", not "a email". A summary is read by a person deciding whether
  // to trust the file, and a sentence that reads wrong undermines that for no
  // reason.
  const noun =
    requirement === "postal" ? "a postal code" : requirement === "email" ? "an email" : "a phone";
  return skipped === 0
    ? `${String(kept)} rows, all with ${noun}`
    : `${String(kept)} rows with ${noun}; ${String(skipped)} skipped for having none`;
}
