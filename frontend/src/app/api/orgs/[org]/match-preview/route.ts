import { NextResponse } from "next/server";

import { forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

const PASSTHROUGH = ["limit"] as const;

/** Steward+: a read-only score-only match preview (§7.16). Thin pass-through to
 * the er-server endpoint, which writes nothing. */
export async function GET(
  request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "steward");
    const incoming = new URL(request.url).searchParams;
    const searchParams = new URLSearchParams();
    for (const key of PASSTHROUGH) {
      const value = incoming.get(key);
      if (value !== null) searchParams.set(key, value);
    }
    const body = await forward<unknown>(access, `/v1/orgs/${org}/match-preview`, { searchParams });
    return NextResponse.json(body);
  });
}
