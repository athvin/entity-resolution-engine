import { NextResponse } from "next/server";

import { listFrom } from "@/lib/bff/envelope";
import { forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

const PASSTHROUGH = ["state", "limit"] as const;

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
    // er-server wraps *this* list in an object (`jobs`), so this unwraps to the
    // bare array the browser contract uses (`useJobs` types it `JobRow[]`, and
    // the e2e specs call `.some()` on it).
    //
    // It does NOT wrap every list, which this comment used to claim. The
    // generated ledger `src/lib/api/envelopes.json` counts six shapes across 43
    // routed GETs — 10 bare, 10 wrapped, 10 record, 10 scalar, 2 page, 1 arrow —
    // with `/runs`, `/audit` and `/webhooks` bare right next to these. Read the
    // ledger for a route rather than generalising from a neighbour; the gate in
    // `tests/unit/bff-envelope-contract.test.ts` checks this file against it.
    //
    // It used to be `forward<unknown[]>(...)` passed straight through, which was
    // a type ASSERTION about an HTTP body TypeScript cannot check: the real
    // backend sent `{ jobs: [...] }`, so the runs page's jobs table rendered
    // nothing and `/api/orgs/{org}/jobs` answered an object where three separate
    // consumers expected an array. Invisible because the mock returned a bare
    // array too — the W0.4a gate compares routes, not response shapes.
    const page = await forward<unknown>(access, `/v1/orgs/${org}/jobs`, { searchParams });
    return NextResponse.json(listFrom(page, "jobs"));
  });
}
