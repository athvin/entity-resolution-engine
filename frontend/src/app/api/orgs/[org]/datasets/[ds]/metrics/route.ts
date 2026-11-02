import { NextResponse } from "next/server";

import { forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";

export async function GET(
  _request: Request,
  { params }: { params: Promise<{ org: string; ds: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds } = await params;
    const access = await requireDatasetAccess(org, ds, "viewer");
    const metrics = await forward<Record<string, unknown>>(
      access,
      `/v1/orgs/${org}/datasets/${ds}/metrics`,
    );
    return NextResponse.json(metrics);
  });
}
