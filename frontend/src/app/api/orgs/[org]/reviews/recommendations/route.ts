import { NextResponse } from "next/server";

import { recommendationsFor } from "@/lib/adjudicate/store";
import { BffFailure, requireOrgAccess, respond } from "@/lib/bff/proxy";

/**
 * How many reviews one lookup may ask about.
 *
 * The inbox pages well under this; the bound exists so a hand-built URL cannot
 * turn one request into an unbounded `IN (…)` against the control plane.
 */
const MAX_IDS = 200;

/**
 * The stored recommendations for a page of reviews, keyed by review id (V6).
 *
 * A batch read rather than one call per row: the inbox renders a page at a
 * time, and a request per row would be a storm against the control-plane
 * Postgres for data that is decorative until a steward looks at it.
 *
 * Viewer, unlike the POST that creates one. Reading what the model already said
 * costs nothing and spends no tokens — and a viewer who can see the pair and
 * its score is not learning anything new from an opinion about it.
 *
 * This does not touch er-server at all, so there is no upstream path and
 * nothing for the envelope ledger to arbitrate: `review_recommendations` is a
 * BFF-owned table, the same as `segments`.
 *
 * Note this static segment SHADOWS the sibling `[reviewId]` route, which is
 * Next.js's documented precedence and is safe here only because review ids are
 * ULIDs — a review could never be called `recommendations`. Worth knowing
 * before adding another static child beside a dynamic one.
 */
export async function GET(
  request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    await requireOrgAccess(org, "viewer");
    const ids = new URL(request.url).searchParams.getAll("review_id").filter((id) => id !== "");
    if (ids.length > MAX_IDS) {
      throw new BffFailure(
        422,
        "validation",
        `at most ${String(MAX_IDS)} review_id values per request`,
      );
    }
    // De-duplicated so a repeated id cannot inflate the query, and because the
    // response is keyed by id anyway — duplicates could never be distinguished.
    return NextResponse.json(await recommendationsFor(org, [...new Set(ids)]));
  });
}
