import { NextResponse } from "next/server";

import { forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

const PASSTHROUGH = ["action", "before_id", "limit"] as const;

/** The tenant-facing audit feed (design §7.20) — admin-gated, like the server. */
export async function GET(
  request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "admin");
    const incoming = new URL(request.url).searchParams;
    const searchParams = new URLSearchParams();
    for (const key of PASSTHROUGH) {
      const value = incoming.get(key);
      if (value !== null) searchParams.set(key, value);
    }
    const rows = await forward<unknown[]>(access, `/v1/orgs/${org}/audit`, {
      role: "admin",
      searchParams,
    });
    return NextResponse.json(rows);
  });
}
