import { NextResponse } from "next/server";

import { datasetPath, forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";

/**
 * One dataset's detail — what the `[ds]` layout's guard renders its
 * provisioning / suspended states from. A name the org does not have is
 * erserver's 404, passed through as-is.
 */
export async function GET(
  _request: Request,
  { params }: { params: Promise<{ org: string; ds: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds } = await params;
    const access = await requireDatasetAccess(org, ds, "viewer");
    const dataset = await forward<unknown>(access, datasetPath(access, ""));
    return NextResponse.json(dataset);
  });
}
