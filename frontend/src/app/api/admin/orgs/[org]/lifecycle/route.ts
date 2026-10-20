import { NextResponse } from "next/server";
import { z } from "zod";

import { operatorFetch, requireSuperAdmin, writeAudit } from "@/lib/bff/admin";
import { BffFailure, requireOrgSlug, respond } from "@/lib/bff/proxy";

const actionSchema = z.object({ action: z.enum(["suspend", "resume"]) });

export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const identity = await requireSuperAdmin();
    const { org } = await params;
    // Raw-interpolated into the upstream path with the operator token
    // attached; these routes do not pass through requireOrgAccess.
    requireOrgSlug(org);
    const parsed = actionSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(422, "validation", "action must be suspend | resume");
    }
    const result = await operatorFetch<{ name: string; state: string }>(
      identity,
      `/v1/orgs/${org}:${parsed.data.action}`,
      { method: "POST" },
    );
    await writeAudit(identity, `org.${parsed.data.action}`, {}, org);
    return NextResponse.json(result);
  });
}
