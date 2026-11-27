import { NextResponse } from "next/server";
import { z } from "zod";

import { BffFailure, forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";

const decisionSchema = z.object({
  fingerprint: z.string().min(1),
  reason: z.string().max(500).optional(),
});

export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string; ds: string; previewId: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds, previewId } = await params;
    const access = await requireDatasetAccess(org, ds, "steward");
    const parsed = decisionSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(422, "validation", "fingerprint is required");
    }
    const result = await forward<unknown>(
      access,
      `/v1/orgs/${org}/datasets/${ds}/merge-previews/${encodeURIComponent(previewId)}/reject`,
      { method: "POST", role: "steward", body: parsed.data },
    );
    return NextResponse.json(result);
  });
}
