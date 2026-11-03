import { NextResponse } from "next/server";

import { forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";

/**
 * Every try of every task of one job, live and archived
 * (batch-orchestration-design §4.2's task list).
 *
 * Viewer-readable: seeing which stage a run is on, and which try it is, is
 * reading the run's own progress — the same thing the job row already shows,
 * at the resolution the ledger records it.
 */
export async function GET(
  _request: Request,
  { params }: { params: Promise<{ org: string; ds: string; jobId: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds, jobId } = await params;
    const access = await requireDatasetAccess(org, ds, "viewer");
    const result = await forward<unknown>(
      access,
      `/v1/orgs/${org}/datasets/${ds}/jobs/${encodeURIComponent(jobId)}/tasks`,
      { role: "viewer" },
    );
    return NextResponse.json(result);
  });
}
