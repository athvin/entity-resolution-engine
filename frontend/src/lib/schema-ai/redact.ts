import type { CanonicalAttribute, ColumnProfile, EntityType } from "@/lib/domain/csv-profile";

/**
 * What the model may see about a column (V8).
 *
 * **The sample file never leaves the machine**, which `csv-profile.ts` states
 * as its own property and this module has to keep true. `ColumnProfile.samples`
 * holds REAL CELL VALUES — a person's actual email, their actual phone number —
 * so sending a profile as-is would ship customer data to a third party to
 * answer a question about column NAMES.
 *
 * So a shape is sent instead of a value: `aaa@aaa.aaa` rather than
 * `jane.doe@acme.com`, `999-999-9999` rather than a real number. The shape is
 * what distinguishes an email column from a name column, which is the whole
 * question being asked — the identity of the person in row 1 is not.
 */
export interface RedactedColumn {
  name: string;
  /** 0–1, rounded: "this column is usually empty" is a real mapping signal. */
  fill_rate: number;
  /** How many distinct values were seen in the sample, capped — not the values. */
  distinct_in_sample: number;
  /** Shapes, never values. See `shapeOf`. */
  shapes: string[];
}

export interface SchemaProfile {
  entity_type: EntityType;
  row_count: number;
  columns: RedactedColumn[];
}

/**
 * One value's SHAPE.
 *
 * Letters collapse to `a`, digits to `9`, and everything else survives as
 * itself — so structure (the `@`, the dots, the dashes) is preserved while
 * content is not. Runs are collapsed so `Jane` and `Jonathan` produce the same
 * shape rather than leaking a length that, combined with a rare surname, can
 * identify someone.
 *
 * This is deliberately lossy in one direction only: two different values can
 * share a shape, but a shape can never be read back to a value.
 */
export function shapeOf(value: string): string {
  let out = "";
  let last = "";
  for (const char of value.trim()) {
    const token = /[a-z]/i.test(char) ? "a" : /[0-9]/.test(char) ? "9" : char;
    // Collapse runs of letters and digits; keep punctuation as written, since
    // `a@a.a` and `a@a.a.a` are genuinely different shapes.
    if (token === last && (token === "a" || token === "9")) continue;
    out += token;
    last = token;
  }
  return out.slice(0, 40);
}

/** How many shapes one column contributes. More would spend tokens to repeat
 * a pattern the first two already showed. */
const SHAPES_PER_COLUMN = 2;

/** Columns in one profile. A file with hundreds of columns is real, but the
 * tail of it is not what a mapping hinges on, and an unbounded list is an
 * unbounded prompt. */
const MAX_COLUMNS = 80;

/**
 * Turn a local profile into what may be sent.
 *
 * Every field here is a statement about SHAPE or COUNT. Nothing carries a value
 * a person could be identified by, which is the property that lets this feature
 * exist at all under the subprocessor question infrastructure.md §18.14 raises.
 */
export function redactProfile(
  columns: ColumnProfile[],
  rowCount: number,
  entityType: EntityType,
): SchemaProfile {
  return {
    entity_type: entityType,
    row_count: rowCount,
    columns: columns.slice(0, MAX_COLUMNS).map((column) => ({
      name: column.name,
      // Two decimals: the signal is "mostly full" versus "mostly empty", and a
      // full-precision rate over a small sample is a row count in disguise.
      fill_rate: Math.round(column.fillRate * 100) / 100,
      distinct_in_sample: column.samples.length,
      shapes: [...new Set(column.samples.map(shapeOf))]
        .filter((shape) => shape !== "")
        .slice(0, SHAPES_PER_COLUMN),
    })),
  };
}

/**
 * The model's proposal: which column feeds which canonical attribute.
 *
 * Returned to the wizard as a SUGGESTION the person confirms — the mapping step
 * already works that way for the regex heuristic, and this changes where a
 * suggestion comes from, not who decides.
 */
export interface MappingSuggestion {
  attribute: CanonicalAttribute;
  column: string;
  /** One short sentence; shown so a reader can disagree with the reasoning. */
  reason: string;
}
