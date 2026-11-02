import { NextResponse } from "next/server";
import { z } from "zod";

import { BffFailure, forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";

const putSchema = z.object({
  pairs: z
    .array(
      z.object({
        variant_a: z.string().min(1).max(100),
        variant_b: z.string().min(1).max(100),
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
    const lexicon = await forward<unknown>(access, `/v1/orgs/${org}/datasets/${ds}/lexicon`);
    return NextResponse.json(lexicon);
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
      throw new BffFailure(422, "validation", "pairs[{variant_a, variant_b}] required");
    }
    const result = await forward<unknown>(access, `/v1/orgs/${org}/datasets/${ds}/lexicon`, {
      method: "PUT",
      role: "steward",
      body: parsed.data,
    });
    return NextResponse.json(result);
  });
}
