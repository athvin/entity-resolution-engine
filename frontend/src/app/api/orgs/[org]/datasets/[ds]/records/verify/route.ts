import { NextResponse } from "next/server";
import { z } from "zod";

import { BffFailure, forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";

// One record or a batch in ONE call (see the protect route for why), with an
// optional attribute: absent verifies the whole row, present one cell (2.9).
const verifySchema = z
  .object({
    record_key: z.string().min(1).optional(),
    record_keys: z.array(z.string().min(1)).optional(),
    attribute: z.string().min(1).optional(),
  })
  .refine((v) => (v.record_key ?? "") !== "" || (v.record_keys?.length ?? 0) > 0, {
    message: "record_key or a non-empty record_keys is required",
  });

/** Steward: mark records — or one attribute of them — steward-verified. The
 *  flag is read-side metadata (it never changes clustering) and surfaces on
 *  the entity view, the member-values lane and the groups verified filter. */
export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string; ds: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds } = await params;
    const access = await requireDatasetAccess(org, ds, "steward");
    const parsed = verifySchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(422, "validation", "record_key or record_keys is required");
    }
    const result = await forward<unknown>(access, `/v1/orgs/${org}/datasets/${ds}/records/verify`, {
      method: "POST",
      role: "steward",
      body: { apply_now: true, ...parsed.data },
    });
    return NextResponse.json(result);
  });
}
