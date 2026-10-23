import { NextResponse } from "next/server";

import { listFrom } from "@/lib/bff/envelope";
import { forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

/**
 * The event catalog, so the subscribe form offers what this system can emit
 * instead of a free-text field.
 *
 * This endpoint exists because a subscription used to be an unvalidated array:
 * a tenant could subscribe to a typo and wait forever. Serving the catalog to
 * the form is the UI half of that fix — the server refuses an unknown event,
 * and the browser never has to guess one.
 */
export async function GET(
  _request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "viewer");
    const page = await forward<unknown>(access, `/v1/orgs/${org}/webhooks/events`);
    return NextResponse.json(listFrom(page, "events"));
  });
}
