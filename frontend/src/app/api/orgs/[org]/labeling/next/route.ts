import { NextResponse } from "next/server";

import { forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

/** The band and page size are the caller's to choose; the threshold is not. */
const PASSTHROUGH = ["min", "max", "limit"] as const;

/**
 * The pairs most worth asking a human about: uncertainty-sampled around the
 * auto-merge threshold, with pairs already labelled excluded.
 *
 * Viewer+ to read, because seeing what the model is unsure about is not a
 * change. Recording the verdict is steward-only and lives in `../../labels`.
 *
 * Read as an object: `src/lib/api/envelopes.json` records this path as a
 * `record` (items, snapshot, threshold, band, coverage), so `items` is one
 * field of several rather than the whole body.
 */
export async function GET(
  request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "viewer");
    const incoming = new URL(request.url).searchParams;
    const searchParams = new URLSearchParams();
    for (const key of PASSTHROUGH) {
      const value = incoming.get(key);
      if (value !== null) searchParams.set(key, value);
    }
    const queue = await forward<unknown>(access, `/v1/orgs/${org}/labeling/next`, {
      searchParams,
    });
    return NextResponse.json(queue);
  });
}
