import { NextResponse } from "next/server";

import { forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";

type Params = { params: Promise<{ org: string; ds: string; previewId: string }> };

export async function GET(_request: Request, { params }: Params): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds, previewId } = await params;
    const access = await requireDatasetAccess(org, ds, "viewer");
    const view = await forward<unknown>(
      access,
      `/v1/orgs/${org}/datasets/${ds}/merge-previews/${encodeURIComponent(previewId)}/restore`,
    );
    return NextResponse.json(view);
  });
}

export async function POST(_request: Request, { params }: Params): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds, previewId } = await params;
    const access = await requireDatasetAccess(org, ds, "admin");
    const view = await forward<unknown>(
      access,
      `/v1/orgs/${org}/datasets/${ds}/merge-previews/${encodeURIComponent(previewId)}/restore`,
      { method: "POST", role: "admin" },
    );
    return NextResponse.json(view, { status: 202 });
  });
}
