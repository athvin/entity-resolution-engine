import { NextResponse } from "next/server";

import { forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";

// The merge dialog's group read (spec 2.8): the union of both review sides'
// entity memberships with values — the merge that would actually land, not
// just the pair the review names.
export async function GET(
  _request: Request,
  { params }: { params: Promise<{ org: string; ds: string; reviewId: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds, reviewId } = await params;
    const access = await requireDatasetAccess(org, ds, "viewer");
    const group = await forward<unknown>(
      access,
      `/v1/orgs/${org}/datasets/${ds}/reviews/${encodeURIComponent(reviewId)}/merge-group`,
    );
    return NextResponse.json(group);
  });
}
