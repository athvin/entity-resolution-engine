import { NextResponse } from "next/server";

import { forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

/**
 * No `cursor`: this route feeds the "did my file do anything?" panel, which
 * wants the latest deliveries and never pages. It used to forward to
 * `/v1/orgs/{org}/imports` — a FORWARD keyset cursor (`id > $after`,
 * ascending) — so `?limit=20` returned the *oldest* 20 receipts and a tenant
 * past 20 ingest batches could never see a newly delivered source here. It now
 * forwards to `/imports/recent`, which answers newest-first.
 */
const PASSTHROUGH = ["source", "limit"] as const;

export async function GET(
  request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "viewer");
    const incoming = new URL(request.url).searchParams;
    const searchParams = new URLSearchParams();
    for (const key of PASSTHROUGH) {
      const value = incoming.get(key);
      if (value !== null) searchParams.set(key, value);
    }
    // `/imports/recent` is a bare array (the shape `/runs` uses). This route's
    // own contract is `{items}` and the panel reads `.items`, so the envelope
    // is constructed here rather than asserted about the upstream body — the
    // W0.5b lesson, where `forward<unknown[]>` on `{jobs: […]}` crashed the
    // dashboard.
    const receipts = await forward<unknown[]>(access, `/v1/orgs/${org}/imports/recent`, {
      searchParams,
    });
    return NextResponse.json({ items: receipts });
  });
}
