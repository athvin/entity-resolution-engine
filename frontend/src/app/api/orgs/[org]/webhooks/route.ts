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
    // The one list endpoint on er-server that really is a bare array —
    // `webhooks_index` returns `Json(json!(out))`, not `{items: …}`. Checked
    // against the handler rather than assumed, because the sibling routes in
    // this directory wrap (`{deliveries}`, `{events}`) and guessing wrong here
    // is the exact defect the jobs route carries a comment about.
    const webhooks = await forward<unknown[]>(access, `/v1/orgs/${org}/webhooks`);
    return NextResponse.json(webhooks);
  });
}

const createSchema = z.object({
  url: z.string().min(1),
  events: z.array(z.string().min(1)).min(1),
  secret: z.string().optional(),
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
      throw new BffFailure(422, "validation", "a url and at least one event are required");
    }
    // The URL is NOT validated here beyond being non-empty: er-server runs it
    // through the outbound-URL guard, which resolves the host and checks every
    // resolved address. Re-implementing a weaker version of that check in the
    // BFF would let a browser learn which internal hosts resolve.
    const created = await forward<unknown>(access, `/v1/orgs/${org}/webhooks`, {
      method: "POST",
      role: "admin",
      body: parsed.data,
    });
    // Passed through whole, because the response carries the signing `secret`
    // and this is the only time it is ever returned — `webhooks_index` never
    // lists it again, like an API key.
    return NextResponse.json(created, { status: 201 });
  });
}
