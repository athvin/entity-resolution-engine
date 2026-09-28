import { NextResponse } from "next/server";

import { forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

export async function GET(
  _request: Request,
  { params }: { params: Promise<{ org: string; version: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, version } = await params;
    const access = await requireOrgAccess(org, "viewer");
    const detail = await forward<unknown>(
      access,
      `/v1/orgs/${org}/config/versions/${encodeURIComponent(version)}`,
    );
    return NextResponse.json(detail);
  });
}
