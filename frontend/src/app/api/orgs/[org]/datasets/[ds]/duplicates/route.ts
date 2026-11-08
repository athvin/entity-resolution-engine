import { NextResponse } from "next/server";

import { forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";

// The filter bar's parameters ride through verbatim; the server validates them.
const PASSTHROUGH = [
  "min_members",
  "max_members",
  "min_confidence",
  "max_confidence",
  "member",
  "matched_by",
  "matched_on",
  "verified",
  "tag",
  "exclude_tag",
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
    const page = await forward<unknown>(access, `/v1/orgs/${org}/datasets/${ds}/duplicates`, {
      searchParams,
    });
    return NextResponse.json(page);
  });
}
