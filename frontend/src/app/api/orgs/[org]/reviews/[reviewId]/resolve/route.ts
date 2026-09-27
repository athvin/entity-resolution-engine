import { NextResponse } from "next/server";
import { z } from "zod";

import { BffFailure, forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

const resolveSchema = z.object({ resolution: z.enum(["match", "no_match", "dismiss"]) });

export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string; reviewId: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, reviewId } = await params;
    const access = await requireOrgAccess(org, "steward");
    const parsed = resolveSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(422, "validation", "resolution must be match | no_match | dismiss");
    }
    const result = await forward<unknown>(
      access,
      `/v1/orgs/${org}/reviews/${encodeURIComponent(reviewId)}:resolve`,
      { method: "POST", role: "steward", body: parsed.data },
    );
    return NextResponse.json(result);
  });
}
