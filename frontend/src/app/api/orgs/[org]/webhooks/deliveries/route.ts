import { NextResponse } from "next/server";

import { listFrom } from "@/lib/bff/envelope";
import { forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

const PASSTHROUGH = ["webhook_id", "state", "limit"] as const;

/**
 * The delivery log: what was sent, to whom, and what came back.
 *
 * Before O3b every emit site POSTed inline and discarded the outcome with
 * `let _ =`, so a subscriber that was briefly unreachable missed the event
 * permanently and nobody could find out. The outbox made the outcomes durable;
 * this is the first surface that shows them to the tenant who owns them.
 */
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
    // `{deliveries: [...]}` on the wire, a bare array to the browser — the
    // convention the jobs route established after shipping the opposite.
    const page = await forward<unknown>(access, `/v1/orgs/${org}/webhooks/deliveries`, {
      searchParams,
    });
    return NextResponse.json(listFrom(page, "deliveries"));
  });
}
