import { NextResponse } from "next/server";

import { listFrom } from "@/lib/bff/envelope";
import { forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

const PASSTHROUGH = ["limit"] as const;

/**
 * The resolution-metric series (O7a's trend data).
 *
 * O7a retained the snapshots specifically because every dashboard number had
 * always been computed LIVE — the product could say what is true now and
 * nothing about the direction of travel, and "is the duplicate rate going up"
 * is the question an operator actually has. The series then had no caller at
 * all: this route and the page behind it are what make it answerable.
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
    // `{snapshot, series}` on the wire, a bare array to the browser — the
    // convention the jobs route established. The snapshot is the pin this read
    // was taken at, which the series itself does not need to carry.
    const page = await forward<unknown>(access, `/v1/orgs/${org}/insights`, { searchParams });
    return NextResponse.json(listFrom(page, "series"));
  });
}
