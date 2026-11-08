import { NextResponse } from "next/server";
import { z } from "zod";

import { BffFailure, forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";

const putSchema = z.object({
  rows: z
    .array(
      z.object({
        word: z.string().min(1).max(100),
        dictionary: z.string().min(1).max(100),
      }),
    )
    .max(2000),
});

export async function GET(
  _request: Request,
  { params }: { params: Promise<{ org: string; ds: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds } = await params;
    const access = await requireDatasetAccess(org, ds, "viewer");
    const rows = await forward<unknown>(access, `/v1/orgs/${org}/datasets/${ds}/ignore-words`);
    return NextResponse.json(rows);
  });
}

export async function PUT(
  request: Request,
  { params }: { params: Promise<{ org: string; ds: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds } = await params;
    const access = await requireDatasetAccess(org, ds, "steward");
    const parsed = putSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(422, "validation", "rows[{word, dictionary}] required");
    }
    const result = await forward<unknown>(access, `/v1/orgs/${org}/datasets/${ds}/ignore-words`, {
      method: "PUT",
      role: "steward",
      body: parsed.data,
    });
    return NextResponse.json(result);
  });
}
