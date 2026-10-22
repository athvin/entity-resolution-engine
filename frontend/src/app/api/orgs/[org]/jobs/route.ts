import { NextResponse } from "next/server";

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
    // er-server wraps every list response in an object — `items` for most,
    // `jobs` here — so this unwraps to the bare array the browser contract uses
    // (`useJobs` types it `JobRow[]`, and the e2e specs call `.some()` on it).
    //
    // It used to be `forward<unknown[]>(...)` passed straight through, which was
    // a type ASSERTION about an HTTP body TypeScript cannot check: the real
    // backend sent `{ jobs: [...] }`, so the runs page's jobs table rendered
    // nothing and `/api/orgs/{org}/jobs` answered an object where three separate
    // consumers expected an array. Invisible because the mock returned a bare
    // array too — the W0.4a gate compares routes, not response shapes.
    const page = await forward<{ jobs?: unknown[] }>(access, `/v1/orgs/${org}/jobs`, {
      searchParams,
    });
    return NextResponse.json(page.jobs ?? []);
  });
}
