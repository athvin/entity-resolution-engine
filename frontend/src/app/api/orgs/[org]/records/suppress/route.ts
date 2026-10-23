import { NextResponse } from "next/server";
import { z } from "zod";

import { BffFailure, forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

// Either shape: one record, or a batch in ONE call. The batch is the point —
// each call with `apply_now` enqueues a correction pass over the whole
// corpus, so a client loop over N records queues N full passes.
const suppressSchema = z
  .object({
    record_key: z.string().min(1).optional(),
    record_keys: z.array(z.string().min(1)).optional(),
  })
  .refine((v) => (v.record_key ?? "") !== "" || (v.record_keys?.length ?? 0) > 0, {
    message: "record_key or a non-empty record_keys is required",
  });

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
      throw new BffFailure(422, "validation", "record_key or record_keys is required");
    }
    const result = await forward<unknown>(access, `/v1/orgs/${org}/records/suppress`, {
      method: "POST",
      role: "steward",
      body: { apply_now: true, ...parsed.data },
    });
    return NextResponse.json(result);
  });
}
