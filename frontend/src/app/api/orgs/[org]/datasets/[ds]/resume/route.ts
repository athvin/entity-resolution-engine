import { NextResponse } from "next/server";

import { datasetPath, forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";

/** Unfreeze one dataset. Admin, matching suspend. */
export async function POST(
  _request: Request,
  { params }: { params: Promise<{ org: string; ds: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds } = await params;
    const access = await requireDatasetAccess(org, ds, "admin");
    const result = await forward<unknown>(access, datasetPath(access, "/resume"), {
      method: "POST",
      role: "admin",
    });
    return NextResponse.json(result);
  });
}
