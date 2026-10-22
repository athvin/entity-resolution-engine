import { NextResponse } from "next/server";

import { forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

/**
 * One model version in full: the settings document and the derived weights view
 * — each comparison level's m/u and the `log2(m/u)` match weight it implies.
 * Those numbers were always in the engine and nothing could read them.
 */
export async function GET(
  _request: Request,
  { params }: { params: Promise<{ org: string; name: string; version: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, name, version } = await params;
    const access = await requireOrgAccess(org, "viewer");
    const detail = await forward<unknown>(
      access,
      `/v1/orgs/${org}/models/${encodeURIComponent(name)}/versions/${encodeURIComponent(version)}`,
    );
    return NextResponse.json(detail);
  });
}
