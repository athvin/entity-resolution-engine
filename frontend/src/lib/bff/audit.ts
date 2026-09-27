import "server-only";
import { ulid } from "ulidx";

import type { Identity } from "@/lib/auth/session";
import { db, schema } from "@/lib/db";

/** One erweb.audit row; dual-identity when the session is impersonating. */
export async function writeAudit(
  identity: Identity,
  action: string,
  detail: Record<string, unknown>,
  actingOrg?: string,
): Promise<void> {
  const database = await db();
  await database.insert(schema.audit).values({
    id: ulid().toLowerCase(),
    userId: identity.user.id,
    actingOrg: actingOrg ?? null,
    actingRole: identity.impersonating
      ? identity.impersonating.role
      : identity.user.isSuperAdmin
        ? "super_admin"
        : "user",
    impersonating: identity.impersonating !== null,
    action,
    detail,
  });
}
