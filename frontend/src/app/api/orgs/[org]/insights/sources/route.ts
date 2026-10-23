import { NextResponse } from "next/server";

import { forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

const PASSTHROUGH = ["run_id"] as const;

/**
 * One run's per-source duplicate attribution.
 *
 * The corpus-wide rate says a tenant has a problem; this says which feed it
 * came from, which is the difference between knowing and being able to act.
 *
 * Passed through WHOLE rather than unwrapped to a bare array, unlike the
 * sibling `/insights` route: `run_id` is part of the answer here. The page
 * shows which run the attribution describes, and a bare array could not say.
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
    const page = await forward<unknown>(access, `/v1/orgs/${org}/insights/sources`, {
      searchParams,
    });
    return NextResponse.json(page);
  });
}
