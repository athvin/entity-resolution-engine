import { NextResponse } from "next/server";

import { forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

export async function GET(
  _request: Request,
  { params }: { params: Promise<{ org: string; reviewId: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, reviewId } = await params;
    const access = await requireOrgAccess(org, "viewer");
    const review = await forward<unknown>(
      access,
      `/v1/orgs/${org}/reviews/${encodeURIComponent(reviewId)}`,
    );
    return NextResponse.json(review);
  });
}
