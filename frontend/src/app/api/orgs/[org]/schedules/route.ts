import { NextResponse } from "next/server";
import { z } from "zod";

import { BffFailure, forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

export async function GET(
  _request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "viewer");
    const schedules = await forward<unknown[]>(access, `/v1/orgs/${org}/schedules`);
    return NextResponse.json(schedules);
  });
}

const createSchema = z.object({
  kind: z.string().min(1),
  cron: z.string().min(1),
  params: z.record(z.string(), z.unknown()).optional(),
});

export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "admin");
    const parsed = createSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(422, "validation", "kind and cron are required");
    }
    const created = await forward<unknown>(access, `/v1/orgs/${org}/schedules`, {
      method: "POST",
      role: "admin",
      body: parsed.data,
    });
    return NextResponse.json(created, { status: 201 });
  });
}
