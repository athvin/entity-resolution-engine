import { NextResponse } from "next/server";

import { forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";

export async function GET(
  _request: Request,
  { params }: { params: Promise<{ org: string; ds: string; version: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds, version } = await params;
    const access = await requireDatasetAccess(org, ds, "viewer");
    const detail = await forward<unknown>(
      access,
      `/v1/orgs/${org}/datasets/${ds}/config/versions/${encodeURIComponent(version)}`,
    );
    return NextResponse.json(detail);
  });
}
