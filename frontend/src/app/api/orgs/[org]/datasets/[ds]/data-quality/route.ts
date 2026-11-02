import { NextResponse } from "next/server";

import { forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";

export async function GET(
  request: Request,
  { params }: { params: Promise<{ org: string; ds: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds } = await params;
    const access = await requireDatasetAccess(org, ds, "viewer");
    const staleDays = new URL(request.url).searchParams.get("stale_days");
    const path = staleDays
      ? `/v1/orgs/${org}/datasets/${ds}/data-quality?stale_days=${encodeURIComponent(staleDays)}`
      : `/v1/orgs/${org}/datasets/${ds}/data-quality`;
    const dq = await forward<Record<string, unknown>>(access, path);
    return NextResponse.json(dq);
  });
}
