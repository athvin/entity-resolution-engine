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
    const versions = await forward<unknown[]>(
      access,
      `/v1/orgs/${org}/datasets/${ds}/config/versions`,
    );
    return NextResponse.json(versions);
  });
}

const draftSchema = z.object({
  yaml: z
    .string()
    .min(1)
    .max(512 * 1024),
});

export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string; ds: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds } = await params;
    const access = await requireDatasetAccess(org, ds, "admin");
    const parsed = draftSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(422, "validation", "a yaml body is required");
    }
    const created = await forward<unknown>(
      access,
      `/v1/orgs/${org}/datasets/${ds}/config/versions`,
      {
        method: "POST",
        role: "admin",
        body: parsed.data,
      },
    );
    return NextResponse.json(created, { status: 201 });
  });
}
