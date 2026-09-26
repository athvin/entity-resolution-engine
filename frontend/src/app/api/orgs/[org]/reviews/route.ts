import { NextResponse } from "next/server";

import { forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

export async function GET(
  request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "viewer");
    const incoming = new URL(request.url).searchParams;
    const searchParams = new URLSearchParams();
    const limit = incoming.get("limit");
    if (limit !== null) searchParams.set("limit", limit);
    const reviews = await forward<unknown[]>(access, `/v1/orgs/${org}/reviews`, { searchParams });
    return NextResponse.json(reviews);
  });
}
