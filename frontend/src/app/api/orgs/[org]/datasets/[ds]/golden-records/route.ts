import { NextResponse } from "next/server";

import { forwardStream, requireDatasetAccess, respond } from "@/lib/bff/proxy";

const PASSTHROUGH = ["q", "cursor", "limit"] as const;

export async function GET(
  request: Request,
  { params }: { params: Promise<{ org: string; ds: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds } = await params;
    const access = await requireDatasetAccess(org, ds, "viewer");
    const incoming = new URL(request.url).searchParams;
    const searchParams = new URLSearchParams();
    for (const key of PASSTHROUGH) {
      const value = incoming.get(key);
      if (value !== null) searchParams.set(key, value);
    }
    // The golden-records data lane is binary Arrow IPC: pass the upstream byte
    // stream through untouched and carry the snapshot + next cursor in headers,
    // so the browser decodes the stream in place through the generated reader.
    const upstream = await forwardStream(access, `/v1/orgs/${org}/datasets/${ds}/golden-records`, {
      searchParams,
    });
    const headers = new Headers({ "content-type": upstream.contentType });
    if (upstream.snapshot !== null) headers.set("x-er-snapshot", upstream.snapshot);
    if (upstream.nextCursor !== null) headers.set("x-er-next-cursor", upstream.nextCursor);
    return new NextResponse(upstream.body, { status: 200, headers });
  });
}
