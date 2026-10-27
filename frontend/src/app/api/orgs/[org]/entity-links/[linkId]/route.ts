import { NextResponse } from "next/server";

import { forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

/** Remove a link (E11b). Steward, for the reason the create route gives. */
export async function DELETE(
  _request: Request,
  { params }: { params: Promise<{ org: string; linkId: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, linkId } = await params;
    const access = await requireOrgAccess(org, "steward");
    const result = await forward<unknown>(
      access,
      `/v1/orgs/${org}/entity-links/${encodeURIComponent(linkId)}`,
      { method: "DELETE" },
    );
    return NextResponse.json(result);
  });
}
