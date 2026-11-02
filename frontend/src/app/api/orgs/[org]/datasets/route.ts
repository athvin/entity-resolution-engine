import { NextResponse } from "next/server";

import { forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

/**
 * The org's datasets — the switcher's source of truth.
 *
 * Viewer, because knowing which resolution domains exist is the price of
 * rendering navigation at all; everything a dataset CONTAINS sits behind the
 * dataset-scoped routes and their own checks.
 */
export async function GET(
  _request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "viewer");
    // Bare array upstream (`list_datasets_route` returns `Json(json!(out))`),
    // like webhooks — checked against the handler rather than assumed.
    const datasets = await forward<unknown[]>(access, `/v1/orgs/${org}/datasets`);
    return NextResponse.json(datasets);
  });
}
