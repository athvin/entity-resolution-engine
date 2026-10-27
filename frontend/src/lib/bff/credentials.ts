import "server-only";
import { and, eq } from "drizzle-orm";

import { db, schema } from "@/lib/db";
import { open } from "@/lib/db/crypto";
import { env } from "@/lib/env";
import type { Membership } from "@/lib/auth/session";
import { BffFailure, type EffectiveOrgAccess } from "./proxy";

/** Decrypt the org's stored service key for exactly the role a call needs. */
export async function credentialForAccess(
  access: EffectiveOrgAccess,
  role: Membership["role"],
): Promise<string> {
  if (access.viaOperator) return env().ERSERVER_OPERATOR_TOKEN;
  const database = await db();
  const rows = await database
    .select()
    .from(schema.orgCredentials)
    .where(and(eq(schema.orgCredentials.org, access.org), eq(schema.orgCredentials.role, role)))
    .limit(1);
  const row = rows[0];
  if (!row) {
    throw new BffFailure(
      503,
      "internal",
      `no stored ${role} credential for ${access.org} — re-register the org from the console`,
    );
  }
  const key = Buffer.from(env().ERWEB_CREDENTIAL_KEY, "base64");
  return open(key, access.org, role, {
    ciphertext: Buffer.from(row.ciphertext),
    nonce: Buffer.from(row.nonce),
  });
}

/**
 * The org's stored VIEWER key, resolved from an org name alone (O11).
 *
 * For the one surface with no session: the embeddable widget, whose caller
 * holds a signed token rather than a cookie.
 *
 * **Viewer, hardcoded, and never the operator token.** `credentialForAccess`
 * falls back to the operator credential when a super admin acts without
 * membership — that fallback must not be reachable from an unauthenticated
 * path, where it would turn a leaked widget token into fleet-wide access. This
 * function takes no role parameter for the same reason: there is no caller that
 * should be able to ask for more.
 */
export async function viewerCredentialFor(org: string): Promise<string> {
  const database = await db();
  const rows = await database
    .select()
    .from(schema.orgCredentials)
    .where(and(eq(schema.orgCredentials.org, org), eq(schema.orgCredentials.role, "viewer")))
    .limit(1);
  const row = rows[0];
  if (!row) {
    throw new BffFailure(503, "internal", `no stored viewer credential for ${org}`);
  }
  const key = Buffer.from(env().ERWEB_CREDENTIAL_KEY, "base64");
  return open(key, org, "viewer", {
    ciphertext: Buffer.from(row.ciphertext),
    nonce: Buffer.from(row.nonce),
  });
}
