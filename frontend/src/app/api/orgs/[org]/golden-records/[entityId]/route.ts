import { NextResponse } from "next/server";

import { forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

export async function GET(
  _request: Request,
  { params }: { params: Promise<{ org: string; entityId: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, entityId } = await params;
    const access = await requireOrgAccess(org, "viewer");
    const detail = await forward<unknown>(
      access,
      `/v1/orgs/${org}/golden-records/${encodeURIComponent(entityId)}`,
    );
    return NextResponse.json(detail);
  });
}
