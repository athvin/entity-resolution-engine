import { NextResponse } from "next/server";

import { forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

export async function DELETE(
  _request: Request,
  { params }: { params: Promise<{ org: string; assertionId: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, assertionId } = await params;
    const access = await requireOrgAccess(org, "steward");
    const result = await forward<unknown>(
      access,
      `/v1/orgs/${org}/assertions/${encodeURIComponent(assertionId)}`,
      { method: "DELETE", role: "steward" },
    );
    return NextResponse.json(result);
  });
}
