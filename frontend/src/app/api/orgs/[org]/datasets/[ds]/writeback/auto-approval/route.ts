import { NextResponse } from "next/server";
import { z } from "zod";

import { BffFailure, forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";

const ruleSchema = z.object({
  connection_id: z.string().min(1),
  object: z.enum(["Lead", "Contact", "Account"]),
  enabled: z.boolean(),
  min_score: z.number().min(0).max(1),
  max_members: z.number().int().min(2),
});

type Params = { params: Promise<{ org: string; ds: string }> };

export async function GET(_request: Request, { params }: Params): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds } = await params;
    const access = await requireDatasetAccess(org, ds, "viewer");
    const rules = await forward<unknown>(
      access,
      `/v1/orgs/${org}/datasets/${ds}/writeback/auto-approval`,
    );
    return NextResponse.json(rules);
  });
}

export async function PUT(request: Request, { params }: Params): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds } = await params;
    const access = await requireDatasetAccess(org, ds, "admin");
    const parsed = ruleSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(
        422,
        "validation",
        "connection_id, object, enabled, min_score (0–1) and max_members (≥2) are required",
      );
    }
    const rule = await forward<unknown>(
      access,
      `/v1/orgs/${org}/datasets/${ds}/writeback/auto-approval`,
      { method: "PUT", role: "admin", body: parsed.data },
    );
    return NextResponse.json(rule);
  });
}
