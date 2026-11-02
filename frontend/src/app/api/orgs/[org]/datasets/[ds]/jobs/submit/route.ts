import { NextResponse } from "next/server";
import { z } from "zod";

import { BffFailure, forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";

const submitSchema = z.object({
  kind: z.enum(["run_all_full", "run_all_incremental", "correct", "train"]),
  params: z.record(z.string(), z.unknown()).optional(),
  /** Client-generated intent id; makes double-taps and mobile retries safe. */
  intent: z.string().min(8).max(64),
});

export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string; ds: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds } = await params;
    const access = await requireDatasetAccess(org, ds, "steward");
    const parsed = submitSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(422, "validation", "kind and intent are required");
    }
    const job = await forward<unknown>(access, `/v1/orgs/${org}/datasets/${ds}/jobs`, {
      method: "POST",
      role: "steward",
      body: { kind: parsed.data.kind, params: parsed.data.params ?? {} },
      idempotencyKey: `ui:${parsed.data.intent}`,
    });
    return NextResponse.json(job, { status: 202 });
  });
}
