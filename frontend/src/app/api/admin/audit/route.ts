import { desc } from "drizzle-orm";
import { NextResponse } from "next/server";

import { operatorFetch, requireSuperAdmin } from "@/lib/bff/admin";
import { respond } from "@/lib/bff/proxy";
import { db, schema } from "@/lib/db";

/**
 * The operator's cross-tenant audit view: the control plane's ledger (every
 * authenticated mutation) plus the app tier's own trail (logins, provisions,
 * impersonation enter/exit and impersonated mutations).
 */
export async function GET(request: Request): Promise<NextResponse> {
  return respond(async () => {
    const identity = await requireSuperAdmin();
    const incoming = new URL(request.url).searchParams;
    const searchParams = new URLSearchParams({ limit: incoming.get("limit") ?? "100" });
    const org = incoming.get("org");
    if (org) searchParams.set("org", org);
    const controlPlane = await operatorFetch<unknown[]>(identity, "/v1/audit", { searchParams });

    const database = await db();
    const app = await database
      .select()
      .from(schema.audit)
      .orderBy(desc(schema.audit.id))
      .limit(100);

    return NextResponse.json({ control_plane: controlPlane, app });
  });
}
