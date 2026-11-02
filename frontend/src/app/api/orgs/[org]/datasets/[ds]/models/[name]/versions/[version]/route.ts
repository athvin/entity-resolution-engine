import { NextResponse } from "next/server";

import { forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";

/**
 * One model version in full: the settings document and the derived weights view
 * — each comparison level's m/u and the `log2(m/u)` match weight it implies.
 * Those numbers were always in the engine and nothing could read them.
 */
export async function GET(
  _request: Request,
  { params }: { params: Promise<{ org: string; ds: string; name: string; version: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds, name, version } = await params;
    const access = await requireDatasetAccess(org, ds, "viewer");
    const detail = await forward<unknown>(
      access,
      `/v1/orgs/${org}/datasets/${ds}/models/${encodeURIComponent(name)}/versions/${encodeURIComponent(version)}`,
    );
    return NextResponse.json(detail);
  });
}
