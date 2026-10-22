import { NextResponse } from "next/server";

import { forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

/**
 * Activate one version. Admin-only, and a separate explicit step from uploading
 * — an upload lands as a draft so authoring a model can never silently
 * re-cluster a tenant's corpus. Scoped to the version's NAME: other names stay
 * active, which is what an ensemble is.
 */
export async function POST(
  _request: Request,
  { params }: { params: Promise<{ org: string; name: string; version: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, name, version } = await params;
    const access = await requireOrgAccess(org, "admin");
    const result = await forward<unknown>(
      access,
      `/v1/orgs/${org}/models/${encodeURIComponent(name)}/versions/${encodeURIComponent(
        version,
      )}/activate`,
      { method: "POST", role: "admin" },
    );
    return NextResponse.json(result);
  });
}
