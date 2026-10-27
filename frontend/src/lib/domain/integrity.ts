import type { ColumnProfile } from "./csv-profile";

/**
 * Pre-load integrity checks for a source file (F2).
 *
 * **Before the upload, not after it.** E9b-1 screens how records landed
 * *against the corpus* once the dispatcher has them — that answers "did my
 * file do anything". This answers the earlier question: "will my file work at
 * all", at the one moment the answer is still cheap, because the file is in
 * the browser and nothing has been written.
 *
 * So this needs no backend and costs no upload. A file whose record-id column
 * has duplicates fails at ingest today, minutes later, in a job receipt — and
 * the person who picked the column has long since moved on.
 */

export type Severity = "block" | "warn" | "info";

export interface Finding {
  severity: Severity;
  /** The column this is about, when it is about one. */
  column: string | null;
  message: string;
  /** How many rows exhibit it, where that is meaningful. */
  rows?: number;
}

/** Rows scanned for the row-level checks. The whole file is already parsed, so
 * this bounds only the per-row work, not the read. */
const SCAN_LIMIT = 5_000;

export interface IntegrityInput {
  columns: ColumnProfile[];
  rowCount: number;
  /** The column chosen as the record id, if one has been. */
  idColumn: string;
  /** Canonical attribute -> source column, as mapped so far. */
  mapping: Record<string, string | undefined>;
  /** Raw rows, for the checks a profile cannot answer.
   *
   * **A SAMPLE, not the file.** `profileCsv` parses the first 200 rows, so a
   * duplicate id beyond that is not seen. Findings from these rows therefore
   * say "in the sampled rows" — reporting a clean result as a guarantee would
   * be exactly the false assurance this panel exists to avoid. */
  rows?: readonly (readonly string[])[];
  /** Header order, needed to index `rows`. */
  header?: readonly string[];
}

/**
 * Everything wrong with this file, worst first.
 *
 * The severities mean something specific and the UI acts on them:
 *
 * - **block** — ingestion will fail or silently corrupt. The wizard refuses to
 *   continue.
 * - **warn** — it will load, but the result will probably not be what the
 *   person expects. They may proceed.
 * - **info** — worth knowing, no action implied.
 *
 * A check that cannot decide between block and warn belongs at warn: blocking
 * a file that would actually have worked teaches people to distrust the gate,
 * and a gate nobody trusts is worse than none.
 */
export function checkIntegrity(input: IntegrityInput): Finding[] {
  const findings: Finding[] = [];
  const { columns, rowCount, idColumn, mapping } = input;

  if (columns.length === 0) {
    return [{ severity: "block", column: null, message: "this file has no header row" }];
  }

  // Duplicate headers: the parser keeps the last, so one column silently
  // shadows another and the mapping points somewhere the reader did not choose.
  const seen = new Set<string>();
  for (const column of columns) {
    if (seen.has(column.name)) {
      findings.push({
        severity: "block",
        column: column.name,
        message: `the column "${column.name}" appears more than once — one would silently shadow the other`,
      });
    }
    seen.add(column.name);
  }

  if (rowCount === 0) {
    findings.push({
      severity: "block",
      column: null,
      message: "this file has a header but no rows",
    });
  }

  // The record id is the one column whose problems corrupt rather than annoy:
  // a blank id cannot address a record, and a duplicate makes two source rows
  // collapse into one, losing data with no error anywhere.
  if (idColumn !== "") {
    const profile = columns.find((column) => column.name === idColumn);
    if (profile === undefined) {
      findings.push({
        severity: "block",
        column: idColumn,
        message: `the record-id column "${idColumn}" is not in this file`,
      });
    } else if (profile.fillRate < 1) {
      const blank = Math.round((1 - profile.fillRate) * rowCount);
      findings.push({
        severity: "block",
        column: idColumn,
        rows: blank,
        message: `the record-id column is empty on some rows — those records cannot be addressed`,
      });
    }

    const duplicates = countDuplicateIds(input);
    if (duplicates > 0) {
      findings.push({
        severity: "block",
        column: idColumn,
        rows: duplicates,
        message: `the record id repeats in the sampled rows — later rows would overwrite earlier ones, losing data with no error`,
      });
    }
  }

  // A mapped column that is mostly empty is not an error, but it is almost
  // always the wrong column: someone mapped `email` to a notes field.
  for (const [attribute, column] of Object.entries(mapping)) {
    if (column === undefined || column === "") continue;
    const profile = columns.find((candidate) => candidate.name === column);
    if (profile === undefined) {
      findings.push({
        severity: "block",
        column,
        message: `"${attribute}" is mapped to "${column}", which is not in this file`,
      });
      continue;
    }
    if (profile.fillRate === 0) {
      findings.push({
        severity: "warn",
        column,
        message: `"${attribute}" is mapped to "${column}", which is empty on every row`,
      });
    } else if (profile.fillRate < 0.1) {
      findings.push({
        severity: "warn",
        column,
        rows: Math.round(profile.fillRate * rowCount),
        message: `"${attribute}" is mapped to "${column}", which is populated on under 10% of rows`,
      });
    }
  }

  // Unmapped columns are kept as metadata rather than dropped, which the
  // wizard says — repeated here because this panel is where someone is
  // deciding whether the file is right.
  const mapped = new Set(Object.values(mapping).filter((value): value is string => Boolean(value)));
  const extras = columns.filter(
    (column) => !mapped.has(column.name) && column.name !== idColumn,
  ).length;
  if (extras > 0) {
    findings.push({
      severity: "info",
      column: null,
      rows: extras,
      message: `${String(extras)} column${extras === 1 ? "" : "s"} are not mapped — they are kept as metadata, not dropped`,
    });
  }

  const order: Record<Severity, number> = { block: 0, warn: 1, info: 2 };
  return findings.sort((a, b) => order[a.severity] - order[b.severity]);
}

/** How many rows repeat an already-seen record id. */
function countDuplicateIds(input: IntegrityInput): number {
  const { rows, header, idColumn } = input;
  if (rows === undefined || header === undefined || idColumn === "") return 0;
  const index = header.indexOf(idColumn);
  if (index < 0) return 0;
  const seen = new Set<string>();
  let duplicates = 0;
  for (const row of rows.slice(0, SCAN_LIMIT)) {
    const value = (row[index] ?? "").trim();
    if (value === "") continue;
    if (seen.has(value)) duplicates += 1;
    else seen.add(value);
  }
  return duplicates;
}

/** Does anything here stop the wizard? */
export function hasBlocker(findings: readonly Finding[]): boolean {
  return findings.some((finding) => finding.severity === "block");
}
