import { eq } from "drizzle-orm";
import { NextResponse } from "next/server";
import { z } from "zod";

import { operatorFetch, requireSuperAdmin, writeAudit } from "@/lib/bff/admin";
import { BffFailure, requireIdentity, respond } from "@/lib/bff/proxy";
import { db, schema } from "@/lib/db";

const IMPERSONATION_MINUTES = 60;

const enterSchema = z.object({
  org: z.string().min(1).max(128),
  role: z.enum(["viewer", "steward", "admin"]),
});

/** Enter "view as tenant": short-TTL state on the session row, audited both ways. */
export async function POST(request: Request): Promise<NextResponse> {
  return respond(async () => {
    const identity = await requireSuperAdmin();
    const parsed = enterSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(422, "validation", "org and role (viewer|steward|admin) are required");
    }
    const { org, role } = parsed.data;
    await operatorFetch<Record<string, unknown>>(identity, `/v1/orgs/${org}`); // 404s if unknown

    const expiresAt = new Date(Date.now() + IMPERSONATION_MINUTES * 60 * 1000);
    const database = await db();
    await database
      .update(schema.sessions)
      .set({
        impersonatingOrg: org,
        impersonatedRole: role,
        impersonationExpiresAt: expiresAt,
      })
      .where(eq(schema.sessions.id, identity.sessionId));
    await writeAudit(identity, "impersonation.start", { role }, org);
    return NextResponse.json({ org, role, expires_at: expiresAt.toISOString() });
  });
}

export async function DELETE(): Promise<NextResponse> {
  return respond(async () => {
    const identity = await requireIdentity();
    const database = await db();
    await database
      .update(schema.sessions)
      .set({
        impersonatingOrg: null,
        impersonatedRole: null,
        impersonationExpiresAt: null,
      })
      .where(eq(schema.sessions.id, identity.sessionId));
    if (identity.impersonating) {
      await writeAudit(
        identity,
        "impersonation.stop",
        { role: identity.impersonating.role },
        identity.impersonating.org,
      );
    }
    return NextResponse.json({ ok: true });
  });
}
