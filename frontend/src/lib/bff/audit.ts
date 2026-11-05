import "server-only";
import { ulid } from "ulidx";

import type { Identity } from "@/lib/auth/session";
import { db, schema } from "@/lib/db";
import { currentRequestContext } from "@/lib/telemetry/context";
import { traceIdOf } from "@/lib/telemetry/traceparent";

/**
 * One erweb.audit row; dual-identity when the session is impersonating.
 *
 * Carries the request's trace id and the browser session, which together make
 * this trail the entry point for "what did this person do, and what did the
 * backend do about it":
 *
 * - `sessionId` comes from the httpOnly cookie via `Identity`, never from a
 *   header or body, so it cannot be forged by a caller. This is the trail that
 *   holds it; erserver's `audit_log` deliberately does not, because er-server
 *   could only learn a session from a header and `x-acting-user` is already
 *   documented as spoofable by any API-key holder by design.
 * - `traceId` is the join key. The same id lands on erserver's `audit_log` row
 *   for the same request and on every span it produced, so one query goes from
 *   a person's action to the SQL statements it ran.
 *
 * Both are nullable and null is a real answer: a handler that does not go
 * through `respond()` has no request context, and attaching a fabricated one
 * would assert a causal link that does not exist.
 */
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
    traceId: traceIdOf(currentRequestContext()?.traceparent),
    sessionId: identity.sessionId,
  });
}
