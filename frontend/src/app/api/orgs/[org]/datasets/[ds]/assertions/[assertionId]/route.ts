import { NextResponse } from "next/server";

import { forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";

export async function DELETE(
  _request: Request,
  { params }: { params: Promise<{ org: string; ds: string; assertionId: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds, assertionId } = await params;
    const access = await requireDatasetAccess(org, ds, "steward");
    const result = await forward<unknown>(
      access,
      `/v1/orgs/${org}/datasets/${ds}/assertions/${encodeURIComponent(assertionId)}`,
      { method: "DELETE", role: "steward" },
    );
    return NextResponse.json(result);
  });
}
