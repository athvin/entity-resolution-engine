import "server-only";
import { and, eq, inArray } from "drizzle-orm";

import { db, schema } from "@/lib/db";
import type { Recommendation } from "./propose";

/** A stored recommendation, as the browser contract carries it. */
export interface StoredRecommendation {
  review_id: string;
  verdict: string;
  confidence: number;
  reasoning: string;
  model: string;
  match_probability: number | null;
  requested_by: string;
  created_at: string;
}

function toWire(row: typeof schema.reviewRecommendations.$inferSelect): StoredRecommendation {
  return {
    review_id: row.reviewId,
    verdict: row.verdict,
    confidence: row.confidence,
    reasoning: row.reasoning,
    model: row.model,
    match_probability: row.matchProbability,
    requested_by: row.requestedBy,
    created_at: row.createdAt.toISOString(),
  };
}

/**
 * Store one recommendation, replacing any previous one for the same review.
 *
 * Replace rather than append: two opinions on one pair is a question the UI
 * cannot answer, and the fresher one was asked for a reason. The history that
 * matters is the audited `resolve` a steward eventually makes, not the drafts
 * the model produced on the way.
 */
export async function saveRecommendation(
  org: string,
  reviewId: string,
  recommendation: Recommendation,
  meta: { model: string; matchProbability: number | null; requestedBy: string },
): Promise<StoredRecommendation> {
  const database = await db();
  const values = {
    org,
    reviewId,
    verdict: recommendation.verdict,
    confidence: recommendation.confidence,
    reasoning: recommendation.reasoning,
    model: meta.model,
    matchProbability: meta.matchProbability,
    requestedBy: meta.requestedBy,
    createdAt: new Date(),
  };
  const [row] = await database
    .insert(schema.reviewRecommendations)
    .values(values)
    .onConflictDoUpdate({
      target: [schema.reviewRecommendations.org, schema.reviewRecommendations.reviewId],
      set: values,
    })
    .returning();
  // The insert returns the row it wrote; falling back to the values keeps the
  // return type honest rather than asserting non-null on a driver detail.
  return row === undefined ? toWire({ ...values }) : toWire(row);
}

/**
 * The recommendations for a set of reviews, keyed by review id.
 *
 * Takes the whole page at once because the inbox renders a page of rows: one
 * query per row would be a request storm against the control-plane Postgres for
 * data that is purely decorative until a steward looks at it. An empty input
 * short-circuits — `inArray` with an empty list is a SQL error in some drivers
 * and a full scan in others, and neither is what "nothing to look up" means.
 */
export async function recommendationsFor(
  org: string,
  reviewIds: string[],
): Promise<Record<string, StoredRecommendation>> {
  if (reviewIds.length === 0) return {};
  const database = await db();
  const rows = await database
    .select()
    .from(schema.reviewRecommendations)
    .where(
      and(
        eq(schema.reviewRecommendations.org, org),
        inArray(schema.reviewRecommendations.reviewId, reviewIds),
      ),
    );
  const byReview: Record<string, StoredRecommendation> = {};
  for (const row of rows) {
    byReview[row.reviewId] = toWire(row);
  }
  return byReview;
}
