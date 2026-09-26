import { NextResponse } from "next/server";

import { forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

export async function GET(
  _request: Request,
  { params }: { params: Promise<{ org: string; jobId: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, jobId } = await params;
    const access = await requireOrgAccess(org, "viewer");
    const job = await forward<unknown>(access, `/v1/orgs/${org}/jobs/${encodeURIComponent(jobId)}`);
    return NextResponse.json(job);
  });
}
