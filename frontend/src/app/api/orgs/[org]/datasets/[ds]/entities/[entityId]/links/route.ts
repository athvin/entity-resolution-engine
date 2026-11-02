import { NextResponse } from "next/server";

import { forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";

/**
 * Every link touching one entity, from either side (E11b).
 *
 * Viewer: seeing that an entity belongs to a household is reading, and the
 * panel is how anyone finds out a link exists at all.
 */
export async function GET(
  _request: Request,
  { params }: { params: Promise<{ org: string; ds: string; entityId: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds, entityId } = await params;
    const access = await requireDatasetAccess(org, ds, "viewer");
    const links = await forward<{ items: unknown[] }>(
      access,
      `/v1/orgs/${org}/datasets/${ds}/entities/${encodeURIComponent(entityId)}/links`,
    );
    return NextResponse.json(links);
  });
}
