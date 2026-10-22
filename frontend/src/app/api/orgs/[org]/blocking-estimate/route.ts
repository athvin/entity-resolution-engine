import { NextResponse } from "next/server";

import { forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

const PASSTHROUGH = ["largest"] as const;

/**
 * What a run would cost, before starting it: candidate pairs against the
 * cartesian total, the marginal pairs each blocking rule adds, and the blocks
 * big enough to dominate the run.
 *
 * Expensive — it opens the org's lake and counts — so the UI fetches it on
 * demand rather than on page load. 409 is a DESIGNED state, not a failure: an
 * org with no model yet has nothing to estimate.
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
    const estimate = await forward<unknown>(access, `/v1/orgs/${org}/blocking-estimate`, {
      searchParams,
    });
    return NextResponse.json(estimate);
  });
}
