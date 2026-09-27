import { NextResponse } from "next/server";
import { z } from "zod";

import { BffFailure, forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

const unmergeSchema = z.object({
  records: z.array(z.string().min(1)).min(1).max(100),
  apply_now: z.boolean().optional(),
});

export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string; entityId: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, entityId } = await params;
    const access = await requireOrgAccess(org, "steward");
    const parsed = unmergeSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(422, "validation", "records[] (member record_keys) is required");
    }
    const result = await forward<unknown>(
      access,
      `/v1/orgs/${org}/golden-records/${encodeURIComponent(entityId)}:unmerge`,
      { method: "POST", role: "steward", body: { apply_now: true, ...parsed.data } },
    );
    return NextResponse.json(result);
  });
}
