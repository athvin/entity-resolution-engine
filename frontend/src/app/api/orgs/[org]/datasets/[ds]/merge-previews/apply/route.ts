import { NextResponse } from "next/server";

import { forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";

export async function POST(
  _request: Request,
  { params }: { params: Promise<{ org: string; ds: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds } = await params;
    const access = await requireDatasetAccess(org, ds, "admin");
    const result = await forward<unknown>(
      access,
      `/v1/orgs/${org}/datasets/${ds}/merge-previews/apply`,
      { method: "POST", role: "admin" },
    );
    return NextResponse.json(result, { status: 202 });
  });
}
