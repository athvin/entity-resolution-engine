import { NextResponse } from "next/server";
import { z } from "zod";

import { BffFailure, forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

/** List (backend batch 2). */
export async function GET(
  request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "viewer");
    const incoming = new URL(request.url).searchParams;
    const searchParams = new URLSearchParams();
    for (const key of ["include_retracted", "limit"]) {
      const value = incoming.get(key);
      if (value !== null) searchParams.set(key, value);
    }
    const assertions = await forward<unknown>(access, `/v1/orgs/${org}/assertions`, {
      searchParams,
    });
    return NextResponse.json(assertions);
  });
}

const createSchema = z.object({
  kind: z.enum(["always", "never"]),
  a: z.string().min(1),
  b: z.string().min(1),
  note: z.string().max(500).optional(),
  apply_now: z.boolean().optional(),
});

export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "steward");
    const parsed = createSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(422, "validation", "kind (always|never), a and b are required");
    }
    const created = await forward<unknown>(access, `/v1/orgs/${org}/assertions`, {
      method: "POST",
      role: "steward",
      body: parsed.data,
    });
    return NextResponse.json(created, { status: 201 });
  });
}
