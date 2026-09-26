import "server-only";
import { ulid } from "ulidx";

import { hashPassword } from "@/lib/auth/password";
import { env } from "@/lib/env";
import { seal } from "./crypto";
import type { Db } from "./index";
import * as schema from "./schema";

/**
 * Deterministic identities for the mock-backed Playwright tier (PGlite only —
 * installed when ERWEB_TEST_FIXTURES=1). The org's "erserver keys" are fake:
 * the mock erserver accepts any bearer, so the vault round-trip is still real.
 */
export const TEST_PASSWORD = "password-123!";

export const TEST_USERS = {
  superAdmin: "root@er.dev",
  admin: "admin@acme.dev",
  steward: "steward@acme.dev",
  viewer: "viewer@acme.dev",
} as const;

export const TEST_ORG = "acme-dev";

export async function installTestFixtures(db: Db): Promise<void> {
  const passwordHash = await hashPassword(TEST_PASSWORD);
  const users = [
    { email: TEST_USERS.superAdmin, displayName: "Root Operator", isSuperAdmin: true },
    { email: TEST_USERS.admin, displayName: "Ada Admin", isSuperAdmin: false },
    { email: TEST_USERS.steward, displayName: "Sam Steward", isSuperAdmin: false },
    { email: TEST_USERS.viewer, displayName: "Vic Viewer", isSuperAdmin: false },
  ].map((user) => ({ ...user, id: ulid().toLowerCase(), passwordHash }));
  await db.insert(schema.users).values(users);

  await db.insert(schema.orgsRegistry).values({ org: TEST_ORG, displayName: "Acme (dev)" });

  const byEmail = new Map<string, string>(users.map((user) => [user.email, user.id]));
  const idOf = (email: string): string => {
    const id = byEmail.get(email);
    if (!id) throw new Error(`fixture user ${email} missing`);
    return id;
  };
  await db.insert(schema.orgMembers).values([
    { userId: idOf(TEST_USERS.admin), org: TEST_ORG, role: "admin" },
    { userId: idOf(TEST_USERS.steward), org: TEST_ORG, role: "steward" },
    { userId: idOf(TEST_USERS.viewer), org: TEST_ORG, role: "viewer" },
  ]);

  const key = Buffer.from(env().ERWEB_CREDENTIAL_KEY, "base64");
  await db.insert(schema.orgCredentials).values(
    (["viewer", "steward", "admin"] as const).map((role) => {
      const sealed = seal(key, TEST_ORG, role, `erk_mock_${role}_secret`);
      return {
        org: TEST_ORG,
        role,
        keyId: `mock-${role}`,
        ciphertext: sealed.ciphertext,
        nonce: sealed.nonce,
      };
    }),
  );
}
