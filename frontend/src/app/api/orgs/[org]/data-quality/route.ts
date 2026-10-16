import { NextResponse } from "next/server";

import { forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

export async function GET(
  request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "viewer");
    const staleDays = new URL(request.url).searchParams.get("stale_days");
    const path = staleDays
      ? `/v1/orgs/${org}/data-quality?stale_days=${encodeURIComponent(staleDays)}`
      : `/v1/orgs/${org}/data-quality`;
    const dq = await forward<Record<string, unknown>>(access, path);
    return NextResponse.json(dq);
  });
}
