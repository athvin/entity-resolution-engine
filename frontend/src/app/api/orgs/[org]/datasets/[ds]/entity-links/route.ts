import { NextResponse } from "next/server";

import { forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";

/**
 * Link two entities without merging them (E11b).
 *
 * **Steward, not admin.** This is the same judgement a steward already makes
 * resolving a review, and it is strictly LESS destructive than the merge they
 * can already perform. Gating it higher would make the cheapest way to record
 * "these are related" a merge — the outcome the feature exists to avoid.
 */
export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string; ds: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds } = await params;
    const access = await requireDatasetAccess(org, ds, "steward");
    const body: unknown = await request.json();
    const link = await forward<unknown>(access, `/v1/orgs/${org}/datasets/${ds}/entity-links`, {
      method: "POST",
      body,
    });
    return NextResponse.json(link, { status: 201 });
  });
}
