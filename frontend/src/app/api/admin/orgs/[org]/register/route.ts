import { NextResponse } from "next/server";

import { mintServiceKeys, operatorFetch, requireSuperAdmin, writeAudit } from "@/lib/bff/admin";
import { requireOrgSlug, respond } from "@/lib/bff/proxy";
import { db, schema } from "@/lib/db";

/** Adopt an org that was provisioned outside the UI: mint + vault service keys. */
export async function POST(
  _request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const identity = await requireSuperAdmin();
    const { org } = await params;
    // Raw-interpolated into the upstream path with the operator token
    // attached; these routes do not pass through requireOrgAccess.
    requireOrgSlug(org);
    await operatorFetch<Record<string, unknown>>(identity, `/v1/orgs/${org}`); // 404s if unknown
    await mintServiceKeys(identity, org);
    const database = await db();
    await database
      .insert(schema.orgsRegistry)
      .values({ org, displayName: org, provisionedByUser: identity.user.id })
      .onConflictDoNothing();
    await writeAudit(identity, "org.register", {}, org);
    return NextResponse.json({ registered: true });
  });
}
