/**
 * Formatters shared by the profile panels.
 *
 * Local to this directory rather than global: every surface in this product
 * formats for its own question, and the insights page's `percent()` already
 * differs from this one (it takes a fraction, this takes a numerator and a
 * denominator, because every rate on this screen is "n of m" and computing the
 * fraction at each call site is how a divide-by-zero gets written four times).
 *
 * The shared rule is the em dash: a number nobody can answer renders `—`, never
 * `0`. A fabricated zero is the worst available answer because it is
 * indistinguishable from a real one.
 */

/** A count with grouped digits. */
export function count(value: number | null): string {
  return value === null ? "—" : value.toLocaleString();
}

/**
 * `n / m` as a percentage, or `—` when the denominator is zero.
 *
 * An empty column is not 0% complete and not 100% complete; it is a question
 * with no answer, and the one thing this must not do is pick one.
 */
export function rate(numerator: number, denominator: number): string {
  if (denominator === 0) return "—";
  return `${((numerator / denominator) * 100).toFixed(1)}%`;
}

/** A fraction already computed, as a percentage to one decimal. */
export function percent(value: number | null, digits = 1): string {
  return value === null ? "—" : `${(value * 100).toFixed(digits)}%`;
}

/** A float to `digits` places, or an em dash. */
export function decimal(value: number | null, digits = 2): string {
  return value === null ? "—" : value.toFixed(digits);
}

/** `YYYY-MM-DD HH:MM` from the server's instant, sliced rather than localised. */
export function stamp(value: string): string {
  return value.replace("T", " ").slice(0, 16);
}

/**
 * A large pair count, abbreviated — `2.1M` rather than `2,140,882`.
 *
 * Blocking costs span ten orders of magnitude, and the comparison a reader is
 * making is "is this key thousands or billions", which grouped digits make them
 * count zeros to answer.
 */
export function magnitude(value: number): string {
  if (value < 1_000) return value.toLocaleString();
  if (value < 1_000_000) return `${(value / 1_000).toFixed(1)}k`;
  if (value < 1_000_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
  return `${(value / 1_000_000_000).toFixed(1)}B`;
}

/**
 * How well a value casts to a type, as the best-fitting type and its rate.
 *
 * Everything arrives as a string, so "this column is a date" is a measurement
 * and not a declaration — and reporting the best fit with its rate is the
 * difference between "date" and "date, 94% of the time", which is the number
 * that tells someone they have 6% of rows about to become silent NULLs.
 */
export function inferredType(column: {
  non_null_count: number;
  cast_bigint: number;
  cast_double: number;
  cast_date: number;
  cast_timestamp: number;
  cast_boolean: number;
}): { label: string; conformance: number | null } {
  const { non_null_count: n } = column;
  if (n === 0) return { label: "—", conformance: null };
  // Ordered most specific first: an integer also casts as a double, and a date
  // also casts as a timestamp, so the looser type would always win a tie and
  // the answer would always be the least informative one available.
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
  return { label: best[0], conformance: best[1] / n };
}
