import { NextResponse } from "next/server";
import { z } from "zod";

import { BffFailure, forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";

const unmatchSchema = z.object({
  record_key: z.string().min(1),
});

/** Steward: unmatch a record from its current group — a durable never-rule
 *  against every current co-member, so the record leaves the group at the
 *  next reconcile and can never re-attach to any of them. Not an unmerge: an
 *  unmerge severs; an unmatch also blocks future regrouping. A 409 means the
 *  record is protected, or the never would cross an always-rule. */
export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string; ds: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds } = await params;
    const access = await requireDatasetAccess(org, ds, "steward");
    const parsed = unmatchSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(422, "validation", "record_key is required");
    }
    const result = await forward<unknown>(
      access,
      `/v1/orgs/${org}/datasets/${ds}/records/unmatch`,
      {
        method: "POST",
        role: "steward",
        body: { apply_now: true, ...parsed.data },
      },
    );
    return NextResponse.json(result);
  });
}
