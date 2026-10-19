import { NextResponse } from "next/server";

import { operatorFetch, requireSuperAdmin } from "@/lib/bff/admin";
import { respond } from "@/lib/bff/proxy";

const PASSTHROUGH = ["org", "state", "limit"] as const;

export async function GET(request: Request): Promise<NextResponse> {
  return respond(async () => {
    const identity = await requireSuperAdmin();
    const incoming = new URL(request.url).searchParams;
    const searchParams = new URLSearchParams();
    for (const key of PASSTHROUGH) {
      const value = incoming.get(key);
      if (value !== null) searchParams.set(key, value);
    }
    const jobs = await operatorFetch<Record<string, unknown>[]>(identity, "/v1/jobs", {
      searchParams,
    });
    // Rows carry org_id (the org's UUIDv7); the console displays the org NAME,
    // joined here from the operator org listing.
    const orgs = await operatorFetch<{ id: string; name: string }[]>(identity, "/v1/orgs");
    const names = new Map(orgs.map((o) => [o.id, o.name]));
    const enriched = jobs.map((job) => ({
      ...job,
      org: names.get(job.org_id as string) ?? job.org_id,
    }));
    return NextResponse.json(enriched);
  });
}
