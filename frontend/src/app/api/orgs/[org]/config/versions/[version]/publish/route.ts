import { NextResponse } from "next/server";

import { forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

export async function POST(
  _request: Request,
  { params }: { params: Promise<{ org: string; version: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, version } = await params;
    const access = await requireOrgAccess(org, "admin");
    const result = await forward<unknown>(
      access,
      `/v1/orgs/${org}/config/versions/${encodeURIComponent(version)}:publish`,
      { method: "POST", role: "admin" },
    );
    return NextResponse.json(result);
  });
}
