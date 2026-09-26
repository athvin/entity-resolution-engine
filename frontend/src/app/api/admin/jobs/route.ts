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
    const jobs = await operatorFetch<unknown[]>(identity, "/v1/jobs", { searchParams });
    return NextResponse.json(jobs);
  });
}
