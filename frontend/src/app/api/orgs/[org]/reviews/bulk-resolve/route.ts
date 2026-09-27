import { NextResponse } from "next/server";
import { z } from "zod";

import { BffFailure, forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

const bulkSchema = z.object({
  items: z
    .array(
      z.object({
        review_id: z.string().min(1),
        resolution: z.enum(["match", "no_match", "dismiss"]),
      }),
    )
    .min(1)
    .max(200),
  apply_now: z.boolean().optional(),
});

export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "steward");
    const parsed = bulkSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(422, "validation", "items[{review_id, resolution}] required");
    }
    const result = await forward<unknown>(access, `/v1/orgs/${org}/reviews:bulk-resolve`, {
      method: "POST",
      role: "steward",
      body: parsed.data,
    });
    return NextResponse.json(result);
  });
}
