import { NextResponse } from "next/server";
import { z } from "zod";

import { BffFailure, forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";

// One record or a batch in ONE call — each `apply_now` call enqueues a
// correction pass over the whole corpus, so a client loop over N records
// would queue N full passes.
const deleteSchema = z
  .object({
    record_key: z.string().min(1).optional(),
    record_keys: z.array(z.string().min(1)).optional(),
  })
  .refine((v) => (v.record_key ?? "") !== "" || (v.record_keys?.length ?? 0) > 0, {
    message: "record_key or a non-empty record_keys is required",
  });

/** Steward: retract a record deletion — the record rejoins the corpus at the
 *  next correction pass. The landing history was never touched, so nothing is
 *  lost by a delete/undelete round trip. */
export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string; ds: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds } = await params;
    const access = await requireDatasetAccess(org, ds, "steward");
    const parsed = deleteSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(422, "validation", "record_key or record_keys is required");
    }
    const result = await forward<unknown>(
      access,
      `/v1/orgs/${org}/datasets/${ds}/records/undelete`,
      {
        method: "POST",
        role: "steward",
        body: { apply_now: true, ...parsed.data },
      },
    );
    return NextResponse.json(result);
  });
}
