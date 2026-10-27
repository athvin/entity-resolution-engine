import { NextResponse } from "next/server";

import { forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

/**
 * This tenant's auto-ingest state (E9c): the watched prefix, and per source
 * what was last read out of it.
 *
 * Viewer, not admin. Reading whether your own file landed is a read, and
 * gating it higher would mean the person who dropped the file — usually not an
 * admin — has to ask someone else whether it arrived. Pointing the poller at a
 * bucket is the privileged half, and that lives on the operator's create-org
 * call, not here.
 *
 * No `PASSTHROUGH`: there is one row per configured source, so there is nothing
 * to page or filter. A `limit` accepted and ignored is the bug the jobs
 * endpoint had.
 */
export async function GET(
  _request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "viewer");
    // The upstream body is already this route's contract — an object with
    // `bucket`, `prefix` and `sources` — so it is forwarded whole rather than
    // re-enveloped. Stated because the neighbouring receipts route does the
    // opposite, and the difference is that `/imports/recent` answers a bare
    // array while this one answers an object.
    const state = await forward<{
      bucket: string | null;
      prefix: string | null;
      sources: unknown[];
    }>(access, `/v1/orgs/${org}/s3-ingest`);
    return NextResponse.json(state);
  });
}
