import { eq } from "drizzle-orm";
import { NextResponse } from "next/server";

import { operatorFetch, requireSuperAdmin } from "@/lib/bff/admin";
import { respond } from "@/lib/bff/proxy";
import { db, schema } from "@/lib/db";

export async function GET(
  _request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const identity = await requireSuperAdmin();
    const { org } = await params;
    const detail = await operatorFetch<Record<string, unknown>>(identity, `/v1/orgs/${org}`);
    const database = await db();
    const credentials = await database
      .select({ role: schema.orgCredentials.role })
      .from(schema.orgCredentials)
      .where(eq(schema.orgCredentials.org, org));
    const registry = await database
      .select()
      .from(schema.orgsRegistry)
      .where(eq(schema.orgsRegistry.org, org))
      .limit(1);
    return NextResponse.json({
      ...detail,
      registered: registry.length > 0,
      display_name: registry[0]?.displayName ?? org,
      has_credentials: credentials.length === 3,
    });
  });
}
