import { NextResponse } from "next/server";

import { forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

/**
 * Compile a draft version from the org's ACTIVE published config (E2's
 * config→model compiler). No body: the config is the input, which is the point
 * — the rules an operator already wrote become a model instead of being
 * re-expressed as m/u probabilities by hand.
 */
export async function POST(
  _request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "admin");
    const created = await forward<unknown>(access, `/v1/orgs/${org}/models/compile`, {
      method: "POST",
      role: "admin",
    });
    return NextResponse.json(created, { status: 201 });
  });
}
