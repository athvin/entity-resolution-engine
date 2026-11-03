import type { ColumnProfileRow, ProfileFindingRow, ProfilePresent } from "@/lib/query/profile";

/**
 * What the model may see about a profiled corpus, and the reason the shape is
 * built here rather than passed through.
 *
 * `redactProfile` (`src/lib/schema-ai/redact.ts`) exists because the browser
 * profile's `samples` field holds real cell values, and sending it as-is would
 * ship customer data to answer a question about column names. The server
 * profile has the same hazard at corpus scale and in **five** places, not one:
 * `top_values` is a column's most frequent real values, `lex_min`/`lex_max` its
 * lexical extremes, `num_min`/`num_max` its numeric ones. On an email column
 * `lex_min` is somebody's address; on a salary column `num_max` is somebody's
 * pay.
 *
 * So this is an allowlist, never a delete-list. Every field below is a count, a
 * rate, a length or a derived statistic — nothing a person could be identified
 * by. A delete-list would have been wrong the moment `column_profile` gained a
 * column, and that is not hypothetical: the server's own gate shipped covering
 * `top_values` alone and let the other four through for exactly that reason.
 */
export interface NarratableColumn {
  name: string;
  /** 0–1, two decimals: the signal is "mostly full" versus "mostly empty". */
  fill_rate: number;
  /** Present-but-empty share, separate because ingest stores `''`, not NULL. */
  blank_rate: number;
  /** Distinct per non-null value: ~1 is an identifier, ~0 a constant. */
  cardinality_ratio: number;
  /** True only when the EXACT count was computed and equals the non-null count. */
  unique: boolean | null;
  len_p50: number | null;
  len_p95: number | null;
  /** The best-fitting type and how often it parses, as a rate. */
  looks_like: string;
  type_conformance: number | null;
  entropy_bits: number | null;
  /** The top value's SHARE — never the value. A skew is the point; which value
   * it is adds nothing the model can use. */
  top_share: number | null;
}

export interface NarratableProfile {
  subject: string;
  row_count: number;
  column_count: number;
  sampled: boolean;
  columns: NarratableColumn[];
  findings: { code: string; severity: string; column: string | null; message: string }[];
  blocking: { key: string; expected_pairs: number; eliminated: number; unreachable: number }[];
}

/** Columns in one prompt. A 300-column corpus is real; its tail is not what a
 * narrative hinges on, and an unbounded list is an unbounded prompt. */
const MAX_COLUMNS = 60;

/** Candidate keys worth sending: the ranking is cheapest-first, and past the
 * first handful every row is a key nobody would choose. */
const MAX_KEYS = 8;

function round(value: number, places = 2): number {
  const scale = 10 ** places;
  return Math.round(value * scale) / scale;
}

/**
 * The best-fitting type and its pass rate.
 *
 * Duplicated from `components/profile/format.ts` deliberately: that one renders
 * for a reader and this one feeds a prompt, and sharing it would couple the
 * wording the model sees to a UI label somebody will reword.
 */
function inferredType(column: ColumnProfileRow): { label: string; conformance: number | null } {
  const n = column.non_null_count;
  if (n === 0) return { label: "empty", conformance: null };
  const candidates: [string, number][] = [
    ["boolean", column.cast_boolean],
    ["integer", column.cast_bigint],
    ["number", column.cast_double],
    ["date", column.cast_date],
    ["timestamp", column.cast_timestamp],
  ];
  let best: [string, number] | null = null;
  for (const candidate of candidates) {
    if (best === null || candidate[1] > best[1]) best = candidate;
  }
  if (best === null || best[1] === 0) return { label: "text", conformance: null };
  return { label: best[0], conformance: round(best[1] / n) };
}

/**
 * Reduce a server profile to what may be sent to a model.
 *
 * Findings travel with their prose, which is safe for a reason worth naming:
 * exactly one rule interpolates a raw value into its message (P004, the
 * placeholder rule) and it draws from a hardcoded list of twelve absence
 * markers — "n/a", "null", "-" — matched by equality, not by pattern. That
 * property is enforced by `er-profile`'s own
 * `a_finding_message_names_placeholders_and_never_real_values`, which fails if
 * a rule ever names something outside that set. Without it this would have to
 * send bare codes, and "P004 on email" is not enough for a model to say
 * anything a person could act on.
 */
export function narratable(profile: ProfilePresent): NarratableProfile {
  return {
    subject: profile.subject,
    row_count: profile.row_count,
    column_count: profile.column_count,
    sampled: profile.sampled,
    columns: profile.columns.slice(0, MAX_COLUMNS).map(narratableColumn),
    findings: profile.findings.map((finding: ProfileFindingRow) => ({
      code: finding.code,
      severity: finding.severity,
      column: finding.column_name,
      message: finding.message,
    })),
    blocking: profile.candidate_keys.slice(0, MAX_KEYS).map((key) => {
      const total = (key.row_count * (key.row_count - 1)) / 2;
      return {
        key: key.key_columns.join(" + "),
        expected_pairs: key.expected_pairs,
        eliminated: total === 0 ? 0 : round(1 - key.expected_pairs / total, 4),
        unreachable: round(key.pair_loss_from_nulls, 4),
      };
    }),
  };
}

function narratableColumn(column: ColumnProfileRow): NarratableColumn {
  const rows = column.row_count;
  const filled = column.non_null_count - column.blank_count;
  const distinct = column.distinct_exact ?? column.distinct_approx;
  const type = inferredType(column);
  return {
    name: column.column_name,
    fill_rate: rows === 0 ? 0 : round(filled / rows),
    blank_rate: rows === 0 ? 0 : round(column.blank_count / rows),
    cardinality_ratio: column.non_null_count === 0 ? 0 : round(distinct / column.non_null_count),
    // Only the EXACT count can answer this. The sketch is wrong in both
    // directions — on this repo's own 1000-row fixture it returns 978 for a
    // genuinely unique column — so a verdict from it would be a guess with a
    // boolean's confidence.
    unique: column.distinct_exact === null ? null : column.distinct_exact === column.non_null_count,
    len_p50: column.len_p50,
    len_p95: column.len_p95,
    looks_like: type.label,
    type_conformance: type.conformance,
    entropy_bits: column.entropy_bits === null ? null : round(column.entropy_bits),
    top_share: column.top_values?.[0]?.share ?? null,
  };
}

/** Fields of a column profile that must never reach the model. Exported so the
 * unit test can assert the serialized payload contains none of them, rather
 * than trusting the allowlist above to have stayed an allowlist. */
export const FORBIDDEN_FIELDS = ["top_values", "lex_min", "lex_max", "num_min", "num_max"] as const;

export function narrateSystemPrompt(): string {
  return [
    "You are reading a data profile for an entity-resolution product and writing a short",
    "briefing for the person who owns the data.",
    "",
    "The profile is STATISTICS ONLY. No values from the data are included, by design.",
    "Never ask for them, never speculate about what specific values are, and never invent",
    "an example value to illustrate a point.",
    "",
    "Write at most four short paragraphs, in this order:",
    "1. What this data looks like overall — size, shape, how complete it is.",
    "2. What will hurt matching, most important first. Be concrete about the cost.",
    "3. What to do about it, as specific actions on named columns.",
    "4. Which column to block on and why, if the blocking figures are present.",
    "",
    "Rules:",
    "- Every claim must trace to a number in the input. If a number is absent, say so",
    "  rather than estimating it.",
    "- `unique: null` means nobody computed the exact count — that is 'unknown', not 'no'.",
    "- `sampled: true` means every figure is an estimate; say so once if it is set.",
    "- Do not restate the findings list; the reader can already see it. Explain what it",
    "  MEANS together.",
    "- No headings, no bullet lists, no markdown. Plain prose.",
    "",
    "Then, if and only if the blocking figures support it, propose blocking rules as a",
    "final line beginning `BLOCKING:` followed by one suggestion per column, comma",
    "separated. These are drafts a person will review, not changes you are making.",
  ].join("\n");
}

export function narrateUserContent(profile: NarratableProfile): string {
  return [
    "Here is the profile as JSON. It contains no values from the data.",
    "",
    JSON.stringify(profile),
  ].join("\n");
}

export interface Narrative {
  /** The prose, with the BLOCKING line removed. */
  summary: string;
  /** The model's draft blocking suggestions, if it offered any. */
  blocking: string[];
}

/**
 * Split the model's answer into prose and draft rules.
 *
 * Returns `null` for an empty answer, which the route reports as "no narrative"
 * rather than as an outage — the safety-refusal case lands here too, and a
 * refusal is not a 502.
 */
export function parseNarrative(text: string): Narrative | null {
  const trimmed = text.trim();
  if (trimmed === "") return null;

  // Walked rather than indexed: `findIndex` then `lines[index]` is the shape
  // where this file's two type checkers disagree — eslint's
  // `no-unnecessary-condition` calls a guard redundant and tsc's
  // `noUncheckedIndexedAccess` calls its absence unsafe. One pass needs neither.
  const prose: string[] = [];
  let marker: string | null = null;
  for (const line of trimmed.split("\n")) {
    if (line.trim().toUpperCase().startsWith("BLOCKING:")) {
      marker = line;
      break; // anything after the marker is not prose and not a rule
    }
    prose.push(line);
  }

  const summary = prose.join("\n").trim();
  // Prose is the point; a reply that was only a BLOCKING line answered a
  // different question than the one asked.
  if (summary === "") return null;
  if (marker === null) return { summary, blocking: [] };
  return {
    summary,
    blocking: marker
      .slice(marker.indexOf(":") + 1)
      .split(",")
      .map((part) => part.trim())
      .filter((part) => part !== ""),
  };
}
