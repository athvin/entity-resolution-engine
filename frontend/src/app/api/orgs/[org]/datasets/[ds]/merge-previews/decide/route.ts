import { NextResponse } from "next/server";
import { z } from "zod";

import { BffFailure, forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";

const batchSchema = z.object({
  decision: z.enum(["approve", "reject"]),
  reason: z.string().max(500).optional(),
  items: z
    .array(z.object({ id: z.string().min(1), fingerprint: z.string().min(1) }))
    .min(1)
    .max(100),
});

export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string; ds: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds } = await params;
    const access = await requireDatasetAccess(org, ds, "steward");
    const parsed = batchSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(422, "validation", "decision and 1–100 items are required");
    }
    const result = await forward<unknown>(
      access,
      `/v1/orgs/${org}/datasets/${ds}/merge-previews/decide`,
      { method: "POST", role: "steward", body: parsed.data },
    );
    return NextResponse.json(result);
  });
}
