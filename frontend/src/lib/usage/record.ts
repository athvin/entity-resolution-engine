import "server-only";
import { and, eq, gte, sql } from "drizzle-orm";
import { ulid } from "ulidx";

import { db, schema } from "@/lib/db";

/**
 * Usage metering (O12b).
 *
 * **Metering, not billing.** No price, no credit balance — the pricing story is
 * per-run on cheap compute, and a `cost` column would be a number nobody could
 * reconcile against an invoice. What a tenant needs is "how much of this am I
 * doing", which is a count.
 *
 * Wave 6 made this worth building: four AI surfaces now call a model on a
 * tenant's behalf, and until now nothing recorded that anywhere. A tenant on
 * its own BYO key especially needs to see its own consumption, because the bill
 * arrives from Anthropic rather than from us.
 */

/**
 * What can be metered.
 *
 * Closed, and every entry maps to a real caller — the rule O3a established for
 * its event catalog: "the catalog had to be made true, not just written". A
 * kind nobody emits is a line on a usage panel that always reads zero, which a
 * reader interprets as "I am not using this" rather than "nothing records it".
 */
export const USAGE_KINDS = [
  "ai_assistant",
  "ai_enrich",
  "ai_adjudicate",
  "ai_config_author",
  "ai_config_explain",
  "ai_schema_map",
] as const;
export type UsageKind = (typeof USAGE_KINDS)[number];

/** What `units` counts for a kind. Tokens for a model call; nothing else yet. */
export const UNIT_FOR: Record<UsageKind, string> = {
  ai_assistant: "tokens",
  ai_enrich: "tokens",
  ai_adjudicate: "tokens",
  ai_config_author: "tokens",
  ai_config_explain: "tokens",
  ai_schema_map: "tokens",
};

/** The `usage` block every Anthropic reply carries. */
export interface TokenUsage {
  input_tokens?: number | null;
  output_tokens?: number | null;
}

/**
 * Total tokens from a reply's usage block.
 *
 * Input and output summed rather than kept apart, because the panel answers
 * "how much" and splitting them invites a reader to price them — which is the
 * billing question this deliberately does not answer. A missing or malformed
 * block yields 0: a metering failure must never fail the request that was
 * actually asked for.
 */
export function tokensFrom(usage: TokenUsage | null | undefined): number {
  if (usage === null || usage === undefined) return 0;
  const input = typeof usage.input_tokens === "number" ? usage.input_tokens : 0;
  const output = typeof usage.output_tokens === "number" ? usage.output_tokens : 0;
  const total = input + output;
  return Number.isFinite(total) && total > 0 ? total : 0;
}

/**
 * Record one metered act.
 *
 * **Never throws.** A ledger write that fails must not fail the feature it is
 * measuring: a tenant whose assistant stops answering because metering is down
 * has been given a worse product in exchange for a number nobody asked for.
 * Failures are logged and dropped, which makes the ledger a floor on usage
 * rather than an exact count — stated here so nobody later treats it as
 * authoritative for an invoice.
 */
export async function meter(
  org: string,
  kind: UsageKind,
  units: number,
  detail?: string,
): Promise<void> {
  if (units <= 0) return;
  try {
    const database = await db();
    await database.insert(schema.usageEvents).values({
      id: ulid(),
      org,
      kind,
      units,
      unit: UNIT_FOR[kind],
      detail: detail ?? null,
    });
  } catch (error) {
    // Deliberately swallowed. See the doc comment.
    console.error("usage metering failed", { org, kind, error });
  }
}

export interface UsageRow {
  kind: string;
  unit: string;
  units: number;
  events: number;
}

/**
 * Usage for one org since a cutoff, grouped by kind.
 *
 * Summed on read rather than kept as a running total: a total is a cache, and
 * this table is the source. At the volumes a single tenant generates, the index
 * on `(org, at)` makes the scan cheap enough that a denormalised counter would
 * be a correctness risk bought for nothing.
 */
export async function usageSince(org: string, since: Date): Promise<UsageRow[]> {
  const database = await db();
  const rows = await database
    .select({
      kind: schema.usageEvents.kind,
      unit: schema.usageEvents.unit,
      // `string | number`, which is the TRUTH: postgres.js returns `sum` and
      // `count` as strings while PGlite returns numbers. Declaring `number`
      // here would be a lie that makes the `Number()` below look redundant —
      // eslint said exactly that, and deleting the conversion would have left
      // string concatenation in the panel's total.
      units: sql<string | number>`sum(${schema.usageEvents.units})`,
      events: sql<string | number>`count(*)`,
    })
    .from(schema.usageEvents)
    .where(and(eq(schema.usageEvents.org, org), gte(schema.usageEvents.at, since)))
    .groupBy(schema.usageEvents.kind, schema.usageEvents.unit);

  return rows
    .map((row) => ({
      kind: row.kind,
      unit: row.unit,
      // `sum` comes back as a string from some drivers and a number from
      // others; normalised here so the wire shape never depends on which.
      units: Number(row.units),
      events: Number(row.events),
    }))
    .sort((a, b) => b.units - a.units);
}
