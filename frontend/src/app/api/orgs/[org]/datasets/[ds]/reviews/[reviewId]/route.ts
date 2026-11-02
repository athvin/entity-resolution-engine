import { NextResponse } from "next/server";

import { forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";

export async function GET(
  _request: Request,
  { params }: { params: Promise<{ org: string; ds: string; reviewId: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds, reviewId } = await params;
    const access = await requireDatasetAccess(org, ds, "viewer");
    const review = await forward<unknown>(
      access,
      `/v1/orgs/${org}/datasets/${ds}/reviews/${encodeURIComponent(reviewId)}`,
    );
    return NextResponse.json(review);
  });
}
