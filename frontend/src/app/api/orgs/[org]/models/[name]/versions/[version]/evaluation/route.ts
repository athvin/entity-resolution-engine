import { NextResponse } from "next/server";

import { forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

/** Only `threshold` is the caller's to choose; everything else is derived. */
const PASSTHROUGH = ["threshold"] as const;

/**
 * How a model version scored against the tenant's own verdicts.
 *
 * The engine could always compute accuracy — `dedupe-core::accuracy` has had
 * `truth_space_from_labels_table` and friends since the start — and nothing
 * could produce a label to measure against until E10b. This is the first
 * surface that answers "can I trust this version" with the tenant's data
 * instead of a number from training.
 *
 * Read as an object, not unwrapped: `src/lib/api/envelopes.json` records this
 * path as `scalar` — a computed object (overall, by_origin, counts, curve) with
 * no single list to pull out. It reads `scalar` rather than `record` because the
 * handler returns `Json(body)` from a value built in `label_evaluation`, so the
 * generator sees no inline object literal to enumerate keys from. Both mean the
 * same thing to a consumer: there is nothing to unwrap.
 */
export async function GET(
  request: Request,
  { params }: { params: Promise<{ org: string; name: string; version: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, name, version } = await params;
    const access = await requireOrgAccess(org, "viewer");
    const incoming = new URL(request.url).searchParams;
    const searchParams = new URLSearchParams();
    for (const key of PASSTHROUGH) {
      const value = incoming.get(key);
      if (value !== null) searchParams.set(key, value);
    }
    const evaluation = await forward<unknown>(
      access,
      `/v1/orgs/${org}/models/${encodeURIComponent(name)}/versions/${encodeURIComponent(
        version,
      )}/evaluation`,
      { searchParams },
    );
    return NextResponse.json(evaluation);
  });
}
