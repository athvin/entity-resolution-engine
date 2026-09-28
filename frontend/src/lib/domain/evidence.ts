/**
 * The engine's evidence blob (review_queue.waterfall / match_scores.evidence)
 * is a FLAT dict straight from the model: `gamma_<column>` comparison levels,
 * `mw_<column>` log2 match weights, optional `mw_tf_adj_<column>` term-frequency
 * adjustments, and the total `match_weight`. This module folds it into
 * per-comparison rows the waterfall chart and the agreement grid can render.
 */

export interface EvidenceRow {
  column: string;
  /** Comparison level the pair landed on (higher = closer agreement). */
  gamma: number | null;
  /** log2 match weight, tf-adjustment folded in when present. */
  weight: number;
  supports: boolean;
}

export interface Evidence {
  rows: EvidenceRow[];
  totalWeight: number | null;
}

const GAMMA_PREFIX = "gamma_";
const WEIGHT_PREFIX = "mw_";
const TF_PREFIX = "mw_tf_adj_";
const TOTAL_KEY = "match_weight";

export function parseEvidence(blob: Record<string, unknown> | null | undefined): Evidence {
  if (!blob) return { rows: [], totalWeight: null };
  const gammas = new Map<string, number>();
  const weights = new Map<string, number>();
  const adjustments = new Map<string, number>();
  let totalWeight: number | null = null;

  for (const [key, raw] of Object.entries(blob)) {
    const value = typeof raw === "number" ? raw : Number(raw);
    if (!Number.isFinite(value)) continue;
    if (key === TOTAL_KEY) {
      totalWeight = value;
    } else if (key.startsWith(TF_PREFIX)) {
      adjustments.set(key.slice(TF_PREFIX.length), value);
    } else if (key.startsWith(WEIGHT_PREFIX)) {
      weights.set(key.slice(WEIGHT_PREFIX.length), value);
    } else if (key.startsWith(GAMMA_PREFIX)) {
      gammas.set(key.slice(GAMMA_PREFIX.length), value);
    }
  }

  const columns = new Set([...gammas.keys(), ...weights.keys()]);
  const rows = [...columns]
    .map((column) => {
      const weight = (weights.get(column) ?? 0) + (adjustments.get(column) ?? 0);
      return {
        column,
        gamma: gammas.get(column) ?? null,
        weight,
        supports: weight >= 0,
      };
    })
    .sort((a, b) => Math.abs(b.weight) - Math.abs(a.weight));
  return { rows, totalWeight };
}

/** "emails match exactly (+4.1)" — the plain-language line per comparison. */
export function describeRow(row: EvidenceRow): string {
  const direction = row.supports ? "supports the match" : "argues against";
  const agreement =
    row.gamma === null
      ? ""
      : row.gamma <= 0
        ? "disagree — "
        : row.gamma === 1
          ? "partially agree — "
          : "agree — ";
  const signed = `${row.weight >= 0 ? "+" : ""}${row.weight.toFixed(1)}`;
  return `${humanize(row.column)}: ${agreement}${direction} (${signed})`;
}

export function humanize(column: string): string {
  return column.replaceAll("_", " ");
}
