import { NextResponse } from "next/server";
import { z } from "zod";

import { BffFailure, forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";

export async function GET(
  _request: Request,
  { params }: { params: Promise<{ org: string; ds: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds } = await params;
    const access = await requireDatasetAccess(org, ds, "viewer");
    const schedules = await forward<unknown[]>(access, `/v1/orgs/${org}/datasets/${ds}/schedules`);
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
  { params }: { params: Promise<{ org: string; ds: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds } = await params;
    const access = await requireDatasetAccess(org, ds, "admin");
    const parsed = createSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(422, "validation", "kind and cron are required");
    }
    const created = await forward<unknown>(access, `/v1/orgs/${org}/datasets/${ds}/schedules`, {
      method: "POST",
      role: "admin",
      body: parsed.data,
    });
    return NextResponse.json(created, { status: 201 });
  });
}
