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
/** Orgs reserved for tests that MUTATE state, one per Playwright device project,
 * so parallel projects never race each other and the pixel-compared acme-dev
 * fixtures stay byte-stable. */
export const TEST_ORG_MUTABLE = "mutable-dev"; // desktop-chromium
export const TRIAGE_ORGS = ["mutable-dev", "mutable-ios", "mutable-android", "fresh-dev"] as const;

export async function installTestFixtures(db: Db): Promise<void> {
  const passwordHash = await hashPassword(TEST_PASSWORD);
  const users = [
    { email: TEST_USERS.superAdmin, displayName: "Root Operator", isSuperAdmin: true },
    { email: TEST_USERS.admin, displayName: "Ada Admin", isSuperAdmin: false },
    { email: TEST_USERS.steward, displayName: "Sam Steward", isSuperAdmin: false },
    { email: TEST_USERS.viewer, displayName: "Vic Viewer", isSuperAdmin: false },
  ].map((user) => ({ ...user, id: ulid().toLowerCase(), passwordHash }));
  await db.insert(schema.users).values(users);

  await db
    .insert(schema.orgsRegistry)
    .values([
      { org: TEST_ORG, displayName: "Acme (dev)" },
      ...TRIAGE_ORGS.map((org) => ({ org, displayName: `Mutable (${org})` })),
    ]);

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
    ...TRIAGE_ORGS.flatMap((org) => [
      { userId: idOf(TEST_USERS.admin), org, role: "admin" as const },
      { userId: idOf(TEST_USERS.steward), org, role: "steward" as const },
    ]),
  ]);

  const key = Buffer.from(env().ERWEB_CREDENTIAL_KEY, "base64");
  await db.insert(schema.orgCredentials).values(
    [TEST_ORG, ...TRIAGE_ORGS].flatMap((org) =>
      (["viewer", "steward", "admin"] as const).map((role) => {
        const sealed = seal(key, org, role, `erk_mock_${role}_secret_${org}`);
        return {
          org,
          role,
          keyId: `mock-${org}-${role}`,
          ciphertext: sealed.ciphertext,
          nonce: sealed.nonce,
        };
      }),
    ),
  );
}
