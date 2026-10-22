import { NextResponse } from "next/server";

import { forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

/**
 * The model registry's index: every version, plus the ACTIVE list — one entry
 * per name, because activation is unique per `(org, name)` rather than per org.
 * A viewer may read what their data is scored against; only an admin changes it.
 */
export async function GET(
  _request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "viewer");
    const index = await forward<unknown>(access, `/v1/orgs/${org}/models`);
    return NextResponse.json(index);
  });
}
