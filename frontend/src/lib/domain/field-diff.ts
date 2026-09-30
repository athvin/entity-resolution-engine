/**
 * Field-by-field comparison of two records over a descriptor list — the pure
 * half of the side-by-side compare grid (design §5.3) and of maintenance
 * before→after previews.
 *
 * Absence is normalized before comparison: `null`, `undefined` and the empty
 * string all render as "no value", and two spellings of no value must not be
 * highlighted as a difference.
 */

import type { AttributeDescriptor } from "@/lib/domain/attributes";

export interface FieldComparison {
  readonly key: string;
  readonly label: string;
  readonly a: string | null;
  readonly b: string | null;
  readonly same: boolean;
}

function normalize(value: unknown): string | null {
  if (value === null || value === undefined) return null;
  if (typeof value === "boolean") return value ? "true" : "false";
  if (typeof value === "number" || typeof value === "bigint") return String(value);
  if (typeof value === "string") {
    const text = value.trim();
    return text === "" ? null : text;
  }
  // Attribute values are scalars; a structured value (a lineage blob passed by
  // mistake) still renders deterministically rather than as [object Object].
  return JSON.stringify(value);
}

export function diffFields(
  descriptors: readonly AttributeDescriptor[],
  a: Record<string, unknown>,
  b: Record<string, unknown>,
): FieldComparison[] {
  return descriptors.map(({ key, label }) => {
    const left = normalize(a[key]);
    const right = normalize(b[key]);
    return { key, label, a: left, b: right, same: left === right };
  });
}

/** The rows worth highlighting: both-empty rows are agreement, not signal. */
export function differingFields(rows: readonly FieldComparison[]): FieldComparison[] {
  return rows.filter((row) => !row.same);
}
