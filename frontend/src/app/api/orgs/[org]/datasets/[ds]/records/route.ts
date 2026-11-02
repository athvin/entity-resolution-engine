import { NextResponse } from "next/server";

import { forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";

// Standardized source records by record_key — the review compare grid's read.
// `key` repeats per record (the erserver route caps the list at 50).
export async function GET(
  request: Request,
  { params }: { params: Promise<{ org: string; ds: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds } = await params;
    const access = await requireDatasetAccess(org, ds, "viewer");
    const incoming = new URL(request.url).searchParams;
    const searchParams = new URLSearchParams();
    for (const value of incoming.getAll("key")) searchParams.append("key", value);
    const page = await forward<unknown>(access, `/v1/orgs/${org}/datasets/${ds}/records`, {
      searchParams,
    });
    return NextResponse.json(page);
  });
}
