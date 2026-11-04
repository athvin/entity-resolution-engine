import { NextResponse } from "next/server";

import { forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";

/**
 * Per-record quality: the score distribution, the per-dimension means, and the
 * worst-scoring records.
 *
 * This endpoint has existed upstream, fully implemented, with **no consumer at
 * all** — no BFF route, no hook, no component, no mock arm — since it shipped.
 * Its own feature plan listed wiring it as a free win and then did not.
 *
 * Not the same as `data-quality`, which this directory sits beside: that one is
 * about ATTRIBUTES and feeds the dashboard's completeness meter. This is about
 * RECORDS, and the distinction is the point — a corpus can be 95% complete per
 * attribute and still hold two hundred records that are individually unusable,
 * which the attribute view can never show.
 *
 * Viewer, because it describes the corpus and writes nothing. The worst-records
 * list is bounded upstream (`limit` is clamped there), so a caller cannot turn
 * this into a full export of the corpus ordered by quality.
 */
export async function GET(
  request: Request,
  { params }: { params: Promise<{ org: string; ds: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds } = await params;
    const access = await requireDatasetAccess(org, ds, "viewer");
    const searchParams = new URLSearchParams();
    const limit = new URL(request.url).searchParams.get("limit");
    if (limit !== null) searchParams.set("limit", limit);
    const quality = await forward<unknown>(access, `/v1/orgs/${org}/datasets/${ds}/quality`, {
      searchParams,
    });
    return NextResponse.json(quality);
  });
}
