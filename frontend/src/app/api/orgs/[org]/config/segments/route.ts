import { NextResponse } from "next/server";

import { forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

/**
 * The named slices of the corpus a run may be restricted to — the published
 * config's `segments:` block, with each predicate alongside its name so an
 * operator choosing how to split a long run can see what each slice covers.
 *
 * Deliberately NOT at `/api/orgs/[org]/segments`: that is the CAMPAIGNS
 * saved-segment store, backed by the erweb database and unrelated to this
 * config block. Two different things called "segment" is a naming accident in
 * the product, and putting them on the same route would turn it into a bug.
 */
export async function GET(
  _request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "viewer");
    const catalog = await forward<unknown>(access, `/v1/orgs/${org}/segments`);
    return NextResponse.json(catalog);
  });
}
