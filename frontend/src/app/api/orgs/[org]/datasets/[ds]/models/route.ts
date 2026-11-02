import { NextResponse } from "next/server";

import { forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";

/**
 * The model registry's index: every version, plus the ACTIVE list — one entry
 * per name, because activation is unique per `(org, name)` rather than per org.
 * A viewer may read what their data is scored against; only an admin changes it.
 */
export async function GET(
  _request: Request,
  { params }: { params: Promise<{ org: string; ds: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds } = await params;
    const access = await requireDatasetAccess(org, ds, "viewer");
    const index = await forward<unknown>(access, `/v1/orgs/${org}/datasets/${ds}/models`);
    return NextResponse.json(index);
  });
}
