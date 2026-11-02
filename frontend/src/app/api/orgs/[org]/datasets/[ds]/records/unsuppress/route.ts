import { NextResponse } from "next/server";
import { z } from "zod";

import { BffFailure, forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";

const unsuppressSchema = z.object({ record_key: z.string().min(1) });

/** Steward: retract a record suppression, restoring it to entity assembly. */
export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string; ds: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds } = await params;
    const access = await requireDatasetAccess(org, ds, "steward");
    const parsed = unsuppressSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(422, "validation", "record_key is required");
    }
    const result = await forward<unknown>(
      access,
      `/v1/orgs/${org}/datasets/${ds}/records/unsuppress`,
      {
        method: "POST",
        role: "steward",
        body: { apply_now: true, ...parsed.data },
      },
    );
    return NextResponse.json(result);
  });
}
