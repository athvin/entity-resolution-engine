import { NextResponse } from "next/server";

import { forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";

/**
 * Retry one task of a job without re-running the whole job
 * (batch-orchestration-design §5.4).
 *
 * The CLOSURE is the server's to compute, not this route's: clearing a stage
 * without clearing what consumed it would leave the lake describing two
 * different runs, so the server walks the compiled graph and clears the task
 * plus its downstream. This route forwards the intent and nothing else.
 */
export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string; ds: string; jobId: string; taskId: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds, jobId, taskId } = await params;
    const access = await requireDatasetAccess(org, ds, "steward");
    // An absent or empty body is the ordinary case — re-run on the task's
    // existing budget. `forward` serializes what it is given, so the parsed
    // value goes through rather than the raw text.
    const raw = await request.text();
    const body: unknown = raw ? JSON.parse(raw) : undefined;
    const result = await forward<unknown>(
      access,
      `/v1/orgs/${org}/datasets/${ds}/jobs/${encodeURIComponent(jobId)}/tasks/${encodeURIComponent(taskId)}/clear`,
      {
        method: "POST",
        role: "steward",
        ...(body === undefined ? {} : { body }),
      },
    );
    return NextResponse.json(result);
  });
}
