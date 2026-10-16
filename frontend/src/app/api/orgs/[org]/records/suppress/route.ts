import { NextResponse } from "next/server";
import { z } from "zod";

import { BffFailure, forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

const suppressSchema = z.object({ record_key: z.string().min(1) });

/** Steward: exclude (suppress) a record from entity assembly. Unlike a source
 *  tombstone, the overlay survives re-ingest. */
export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "steward");
    const parsed = suppressSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(422, "validation", "record_key is required");
    }
    const result = await forward<unknown>(access, `/v1/orgs/${org}/records/suppress`, {
      method: "POST",
      role: "steward",
      body: { apply_now: true, ...parsed.data },
    });
    return NextResponse.json(result);
  });
}
