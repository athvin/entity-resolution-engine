import { parseDocument, type Document } from "yaml";

/**
 * The config studio's engine-facing seam: read the structured view out of a
 * tenant config document and apply the studio's edits back onto it, preserving
 * every untouched byte (comments, ordering, the operator-only blocks).
 *
 * The server stays authoritative for validation and tier classification; the
 * client mirror below only powers the "this will cost a ⟨tier⟩ rebuild" label
 * before publish.
 */

export interface Thresholds {
  auto_merge: number;
  review_low: number;
}

export interface BlockingRule {
  key_type: string;
  expr: string;
}

export interface ComparisonSpec {
  levels: string[];
  tf: boolean;
}

export interface ConfigView {
  tenant: string | null;
  thresholds: Thresholds | null;
  survivorship: Record<string, string[]>;
  blocking: BlockingRule[];
  comparisons: Record<string, ComparisonSpec>;
  sources: string[];
}

/** Survivorship rules the engine knows; the pill palette. */
export const SURVIVORSHIP_RULES = [
  "validated",
  "source_priority",
  "recency",
  "frequency",
  "completeness",
] as const;

/** The engine's non-parametric comparison level tokens (S4.3.1). */
export const COMPARISON_LEVELS = [
  "exact",
  "username_exact",
  "variant_match",
  "dob_same_year_month",
] as const;

/** The one parameterized level: `jaro_winkler:<T>` with 0 < T <= 1. */
export const JARO_PREFIX = "jaro_winkler:";

/** The implicit-null level token; always rendered last, never reordered. */
export const NULL_LEVEL = "null";

/** Whether one level token is in the engine's S4.3.1 vocabulary. */
export function isComparisonLevel(token: string): boolean {
  if (token === NULL_LEVEL) return true;
  if ((COMPARISON_LEVELS as readonly string[]).includes(token)) return true;
  if (!token.startsWith(JARO_PREFIX)) return false;
  const threshold = Number(token.slice(JARO_PREFIX.length));
  return Number.isFinite(threshold) && threshold > 0 && threshold <= 1;
}

/** The standardized columns a blocking expr or comparison may reference. */
export const MATCHING_COLUMNS = [
  "given_name",
  "family_name",
  "name_variants",
  "email",
  "email_valid",
  "phone_e164",
  "phone_valid",
  "addr_number",
  "addr_street",
  "addr_unit",
  "addr_city",
  "addr_region",
  "addr_postal",
  "birth_date",
] as const;

const SQL_WORDS = new Set([
  "and",
  "as",
  "between",
  "case",
  "distinct",
  "else",
  "end",
  "false",
  "in",
  "is",
  "like",
  "not",
  "null",
  "or",
  "then",
  "true",
  "when",
]);

/**
 * Advisory client-side checks on one blocking rule. The server's 422 with a
 * JSON pointer stays authoritative; these exist so the obvious mistakes never
 * leave the editor.
 */
export function blockingWarnings(rule: BlockingRule): string[] {
  const warnings: string[] = [];
  if (!rule.key_type.trim()) warnings.push("key_type is empty");
  if (!rule.expr.trim()) {
    warnings.push("expr is empty");
    return warnings;
  }
  let depth = 0;
  for (const char of rule.expr) {
    if (char === "(") depth += 1;
    if (char === ")") depth -= 1;
    if (depth < 0) break;
  }
  if (depth !== 0) warnings.push("parentheses are unbalanced");
  const identifiers = rule.expr
    .replaceAll(/'[^']*'/g, " ")
    .match(/[A-Za-z_][A-Za-z0-9_]*/g);
  const columns = (identifiers ?? []).filter(
    (word) => !SQL_WORDS.has(word.toLowerCase()),
  );
  if (
    columns.length > 0 &&
    !columns.some((word) => (MATCHING_COLUMNS as readonly string[]).includes(word))
  ) {
    warnings.push("expr references no known standardized column");
  }
  return warnings;
}

export function parseConfig(yamlText: string): ConfigView {
  const doc = parseDocument(yamlText);
  const json: unknown = doc.toJS();
  const root = typeof json === "object" && json !== null ? (json as Record<string, unknown>) : {};

  const thresholdsRaw = root.thresholds as { auto_merge?: unknown; review_low?: unknown } | null;
  const thresholds =
    thresholdsRaw &&
    typeof thresholdsRaw.auto_merge === "number" &&
    typeof thresholdsRaw.review_low === "number"
      ? { auto_merge: thresholdsRaw.auto_merge, review_low: thresholdsRaw.review_low }
      : null;

  const survivorship: Record<string, string[]> = {};
  if (typeof root.survivorship === "object" && root.survivorship !== null) {
    for (const [attribute, chain] of Object.entries(root.survivorship)) {
      if (Array.isArray(chain)) survivorship[attribute] = chain.map(String);
    }
  }

  const blocking: BlockingRule[] = Array.isArray(root.blocking)
    ? root.blocking
        .filter(
          (rule): rule is { key_type: unknown; expr: unknown } =>
            typeof rule === "object" && rule !== null,
        )
        .map((rule) => ({ key_type: String(rule.key_type), expr: String(rule.expr) }))
    : [];

  const comparisons: Record<string, ComparisonSpec> = {};
  if (typeof root.comparisons === "object" && root.comparisons !== null) {
    for (const [column, spec] of Object.entries(root.comparisons)) {
      if (
        typeof spec === "object" &&
        spec !== null &&
        Array.isArray((spec as { levels?: unknown }).levels)
      ) {
        comparisons[column] = {
          levels: (spec as { levels: unknown[] }).levels.map((level) =>
            level === null
              ? "null"
              : typeof level === "string" || typeof level === "number"
                ? String(level)
                : JSON.stringify(level),
          ),
          tf: Boolean((spec as { tf?: unknown }).tf),
        };
      }
    }
  }

  const sources =
    typeof root.sources === "object" && root.sources !== null ? Object.keys(root.sources) : [];

  return {
    tenant: typeof root.tenant === "string" ? root.tenant : null,
    thresholds,
    survivorship,
    blocking,
    comparisons,
    sources,
  };
}

export interface StudioEdits {
  thresholds?: Thresholds;
  survivorship?: Record<string, string[]>;
  blocking?: BlockingRule[];
  comparisons?: Record<string, ComparisonSpec>;
}

/** Apply the studio's edits onto the document, byte-preserving everything else.
 * A section absent from `edits` is never touched — the studio passes only what
 * actually changed, so an untouched block cannot cost its rebuild tier. */
export function applyEdits(yamlText: string, edits: StudioEdits): string {
  const doc: Document = parseDocument(yamlText);
  if (edits.thresholds) {
    doc.setIn(["thresholds", "auto_merge"], edits.thresholds.auto_merge);
    doc.setIn(["thresholds", "review_low"], edits.thresholds.review_low);
  }
  if (edits.survivorship) {
    for (const [attribute, chain] of Object.entries(edits.survivorship)) {
      const node = doc.createNode(chain);
      // Keep the config's one-line list style for survivorship chains.
      (node as { flow?: boolean }).flow = true;
      doc.setIn(["survivorship", attribute], node);
    }
  }
  if (edits.blocking) {
    doc.setIn(
      ["blocking"],
      doc.createNode(
        edits.blocking.map((rule) => ({ key_type: rule.key_type, expr: rule.expr })),
      ),
    );
  }
  if (edits.comparisons) {
    for (const [column, spec] of Object.entries(edits.comparisons)) {
      const levels = doc.createNode(
        // "null" is the editor's spelling of the YAML null token.
        spec.levels.map((level) => (level === NULL_LEVEL ? null : level)),
      );
      (levels as { flow?: boolean }).flow = true;
      doc.setIn(["comparisons", column, "levels"], levels);
      doc.setIn(["comparisons", column, "tf"], spec.tf);
    }
  }
  return doc.toString();
}

/** Append a new `sources.<name>` block (the wizard's commit step). */
export function addSourceBlock(
  yamlText: string,
  name: string,
  spec: Record<string, unknown>,
): string {
  const doc: Document = parseDocument(yamlText);
  if (doc.getIn(["sources", name]) !== undefined) {
    throw new Error(`source ${name} already exists in the config`);
  }
  doc.setIn(["sources", name], doc.createNode(spec));
  return doc.toString();
}

/** Client-side mirror of the server's tier table — display only, never enforced. */
export function tierForEdits(edits: StudioEdits): "A" | "B" | "C" | null {
  if (edits.comparisons && Object.keys(edits.comparisons).length > 0) return "C";
  if (edits.blocking) return "B";
  if (edits.thresholds || edits.survivorship) return "A";
  return null;
}

/** The publish button's cost copy per tier — one spelling, tests pin it. */
export const TIER_COST_COPY: Record<"A" | "B" | "C", string> = {
  A: "tier A (re-band + re-assemble)",
  B: "tier B (match rebuild)",
  C: "tier C (retrain + full rebuild)",
};

export interface DiffLine {
  kind: "same" | "added" | "removed";
  text: string;
}

/** A minimal LCS line diff for version comparison (§7.18, client-side v1). */
export function diffLines(before: string, after: string): DiffLine[] {
  const a = before.split("\n");
  const b = after.split("\n");
  const at = (grid: number[][], i: number, j: number): number => grid[i]?.[j] ?? 0;
  const lcs: number[][] = Array.from({ length: a.length + 1 }, () =>
    new Array<number>(b.length + 1).fill(0),
  );
  for (let i = a.length - 1; i >= 0; i -= 1) {
    for (let j = b.length - 1; j >= 0; j -= 1) {
      const row = lcs[i];
      if (!row) continue;
      row[j] =
        a[i] === b[j] ? at(lcs, i + 1, j + 1) + 1 : Math.max(at(lcs, i + 1, j), at(lcs, i, j + 1));
    }
  }
  const out: DiffLine[] = [];
  let i = 0;
  let j = 0;
  while (i < a.length && j < b.length) {
    if (a[i] === b[j]) {
      out.push({ kind: "same", text: a[i] ?? "" });
      i += 1;
      j += 1;
    } else if (at(lcs, i + 1, j) >= at(lcs, i, j + 1)) {
      out.push({ kind: "removed", text: a[i] ?? "" });
      i += 1;
    } else {
      out.push({ kind: "added", text: b[j] ?? "" });
      j += 1;
    }
  }
  while (i < a.length) out.push({ kind: "removed", text: a[i++] ?? "" });
  while (j < b.length) out.push({ kind: "added", text: b[j++] ?? "" });
  return out;
}
