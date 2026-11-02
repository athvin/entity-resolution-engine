import { NextResponse } from "next/server";

import { forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";

/**
 * Activate one version. Admin-only, and a separate explicit step from uploading
 * — an upload lands as a draft so authoring a model can never silently
 * re-cluster a tenant's corpus. Scoped to the version's NAME: other names stay
 * active, which is what an ensemble is.
 */
export async function POST(
  _request: Request,
  { params }: { params: Promise<{ org: string; ds: string; name: string; version: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds, name, version } = await params;
    const access = await requireDatasetAccess(org, ds, "admin");
    const result = await forward<unknown>(
      access,
      `/v1/orgs/${org}/datasets/${ds}/models/${encodeURIComponent(name)}/versions/${encodeURIComponent(
        version,
      )}/activate`,
      { method: "POST", role: "admin" },
    );
    return NextResponse.json(result);
  });
}
