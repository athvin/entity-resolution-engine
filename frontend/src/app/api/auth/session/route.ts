import { NextResponse } from "next/server";

import { requireIdentity, respond } from "@/lib/bff/proxy";

export async function GET(): Promise<NextResponse> {
  return respond(async () => {
    const identity = await requireIdentity();
    return NextResponse.json({
      user: identity.user,
      memberships: identity.memberships,
      impersonating: identity.impersonating,
    });
  });
}
