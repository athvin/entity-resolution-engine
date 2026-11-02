import { NextResponse } from "next/server";

import { forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";

// `status` and `reason` were declared here from the start and IGNORED by the
// server until O6 — a contract this side stated and the backend never
// fulfilled, which is why the band parameters below arrive with a test that
// watches them narrow a real page rather than merely reach the server.
const PASSTHROUGH = [
  "status",
  "reason",
  "min_probability",
  "max_probability",
  "cursor",
  "limit",
] as const;

export async function GET(
  request: Request,
  { params }: { params: Promise<{ org: string; ds: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds } = await params;
    const access = await requireDatasetAccess(org, ds, "viewer");
    const incoming = new URL(request.url).searchParams;
    const searchParams = new URLSearchParams();
    for (const key of PASSTHROUGH) {
      const value = incoming.get(key);
      if (value !== null) searchParams.set(key, value);
    }
    const page = await forward<unknown>(access, `/v1/orgs/${org}/datasets/${ds}/reviews`, {
      searchParams,
    });
    return NextResponse.json(page);
  });
}
