import { NextResponse } from "next/server";

import { forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";

/** Remove a link (E11b). Steward, for the reason the create route gives. */
export async function DELETE(
  _request: Request,
  { params }: { params: Promise<{ org: string; ds: string; linkId: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds, linkId } = await params;
    const access = await requireDatasetAccess(org, ds, "steward");
    const result = await forward<unknown>(
      access,
      `/v1/orgs/${org}/datasets/${ds}/entity-links/${encodeURIComponent(linkId)}`,
      { method: "DELETE" },
    );
    return NextResponse.json(result);
  });
}
