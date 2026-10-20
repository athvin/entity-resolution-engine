import { beforeAll, beforeEach, describe, expect, it } from "vitest";

/**
 * Membership management, against a real database.
 *
 * `src/lib/bff/members.ts` is the largest module in src/lib (349 LOC) and sat at
 * 0% coverage, which matters because the rules it enforces are the ones that
 * decide who can get into a workspace and who can be locked out of one: the
 * last-admin guard, single-use invites, and single-use password resets.
 *
 * These run against **PGlite** — in-process WASM Postgres — rather than mocks.
 * `db()` already supports it (`ERWEB_DATABASE_URL=pglite://memory` applies the
 * committed drizzle migrations on first use), and the Playwright tier relies on
 * the same path. Mocking drizzle here would test the mock: the last-admin guard
 * is a COUNT query, so it is only meaningfully exercised against real SQL.
 *
 * Env is set before any import, because `env()` parses `process.env` lazily on
 * first use and caches it for the process.
 */

process.env.ERWEB_DATABASE_URL = "pglite://memory";
process.env.ERWEB_SESSION_SECRET = "test-session-secret-test-session-secret";
process.env.ERWEB_CREDENTIAL_KEY = "3q2+7wEirykuXBXRO26AY9nbZAqAnDzws76jd2GxwkE=";
process.env.ERSERVER_OPERATOR_TOKEN = "test-operator-token";
process.env.ERWEB_PUBLIC_BASE_URL = "https://app.test";
// Deliberately NOT set: the fixtures install a whole tenant, and these tests
// build exactly the memberships each case needs.
delete process.env.ERWEB_TEST_FIXTURES;

const { db, schema } = await import("@/lib/db");
const { BffFailure } = await import("@/lib/bff/proxy");
const members = await import("@/lib/bff/members");

const ORG = "acme-dev";
let seq = 0;

/**
 * A stand-in for argon2. These functions take the hasher injected, which is
 * what makes this possible: real argon2id is 19 MiB and two passes BY DESIGN,
 * so calling it once per case would dominate the suite's runtime for no added
 * confidence — auth/password.ts owns those parameters, not this module.
 */
const fakeHasher = (password: string): Promise<string> => Promise.resolve(`fake-hash:${password}`);

/** A user row, with a unique email per call so cases cannot collide. */
async function makeUser(overrides: Partial<{ email: string; isSuperAdmin: boolean }> = {}) {
  seq += 1;
  const id = `user-${String(seq).padStart(4, "0")}`;
  const email = overrides.email ?? `user${String(seq)}@acme.dev`;
  const database = await db();
  await database.insert(schema.users).values({
    id,
    email,
    displayName: `User ${String(seq)}`,
    passwordHash: "argon2-placeholder",
    isSuperAdmin: overrides.isSuperAdmin ?? false,
  });
  return { id, email };
}

async function addMember(userId: string, role: "viewer" | "steward" | "admin", org = ORG) {
  const database = await db();
  // No `id` column: org_members is keyed on (org, user_id).
  await database.insert(schema.orgMembers).values({ org, userId, role });
}

/** The status a BffFailure denied with, or "ok". */
async function status(run: () => Promise<unknown>): Promise<number | "ok"> {
  try {
    await run();
    return "ok";
  } catch (error) {
    if (error instanceof BffFailure) return error.status;
    throw error;
  }
}

beforeAll(async () => {
  // Forces connect() → migrate() once, so the first case does not pay for it.
  await db();
});

beforeEach(async () => {
  const database = await db();
  await database.delete(schema.orgMembers);
  await database.delete(schema.invites);
  await database.delete(schema.passwordResets);
  await database.delete(schema.sessions);
  await database.delete(schema.users);
  seq = 0;
});

describe("the last-admin guard", () => {
  it("refuses to demote the only admin of a workspace", async () => {
    const admin = await makeUser();
    await addMember(admin.id, "admin");
    // 409: a request the caller could legitimately make, refused by the state it
    // would produce — leaving a workspace nobody can administer.
    expect(await status(() => members.changeRole(ORG, admin.id, "viewer"))).toBe(409);
  });

  it("refuses to remove the only admin of a workspace", async () => {
    const admin = await makeUser();
    await addMember(admin.id, "admin");
    expect(await status(() => members.removeMember(ORG, admin.id))).toBe(409);
  });

  it("allows demoting an admin once a second admin exists", async () => {
    const first = await makeUser();
    const second = await makeUser();
    await addMember(first.id, "admin");
    await addMember(second.id, "admin");
    expect(await status(() => members.changeRole(ORG, first.id, "viewer"))).toBe("ok");

    const rows = await members.listMembers(ORG);
    expect(rows.members.find((row) => row.user_id === first.id)?.role).toBe("viewer");
    expect(rows.members.find((row) => row.user_id === second.id)?.role).toBe("admin");
  });

  it("allows removing an admin once a second admin exists", async () => {
    const first = await makeUser();
    const second = await makeUser();
    await addMember(first.id, "admin");
    await addMember(second.id, "admin");
    expect(await status(() => members.removeMember(ORG, first.id))).toBe("ok");
    expect((await members.listMembers(ORG)).members.map((row) => row.user_id)).toEqual([second.id]);
  });

  it("does not count admins of OTHER orgs toward this org's last admin", async () => {
    // The guard must be scoped by org, or an admin elsewhere would make it look
    // safe to strip this workspace's only one.
    const here = await makeUser();
    const elsewhere = await makeUser();
    await addMember(here.id, "admin");
    await addMember(elsewhere.id, "admin", "globex-dev");
    expect(await status(() => members.changeRole(ORG, here.id, "viewer"))).toBe(409);
  });

  it("lets a steward or viewer be demoted or removed freely", async () => {
    const admin = await makeUser();
    const steward = await makeUser();
    await addMember(admin.id, "admin");
    await addMember(steward.id, "steward");
    expect(await status(() => members.changeRole(ORG, steward.id, "viewer"))).toBe("ok");
    expect(await status(() => members.removeMember(ORG, steward.id))).toBe("ok");
  });

  it("404s a user who is not a member of this org", async () => {
    const stranger = await makeUser();
    expect(await status(() => members.changeRole(ORG, stranger.id, "viewer"))).toBe(404);
    expect(await status(() => members.removeMember(ORG, stranger.id))).toBe(404);
  });
});

describe("invites", () => {
  it("round-trips: create, read the preview, accept", async () => {
    const inviter = await makeUser();
    await addMember(inviter.id, "admin");

    const created = await members.createInvite(ORG, "newcomer@acme.dev", "steward", inviter.id);
    expect(created.token).toBeTruthy();

    const preview = await members.readInvite(created.token);
    expect(preview.org).toBe(ORG);
    expect(preview.email).toBe("newcomer@acme.dev");

    const accepted = await members.acceptInvite(
      created.token,
      "a-strong-password-1!",
      "Newcomer",
      fakeHasher,
    );
    expect(accepted.org).toBe(ORG);

    const rows = await members.listMembers(ORG);
    expect(rows.members.find((row) => row.email === "newcomer@acme.dev")?.role).toBe("steward");
  });

  it("stores only the token's hash, never the token", async () => {
    const inviter = await makeUser();
    await addMember(inviter.id, "admin");
    const created = await members.createInvite(ORG, "hash@acme.dev", "viewer", inviter.id);

    const database = await db();
    const stored = await database.select().from(schema.invites);
    // A leaked invites table must not be a set of usable invite links.
    expect(stored).toHaveLength(1);
    expect(JSON.stringify(stored)).not.toContain(created.token);
    expect(stored[0]?.tokenHash).toBe(members.hashToken(created.token));
  });

  it("cannot be accepted twice", async () => {
    const inviter = await makeUser();
    await addMember(inviter.id, "admin");
    const created = await members.createInvite(ORG, "once@acme.dev", "viewer", inviter.id);
    await members.acceptInvite(created.token, "a-strong-password-1!", "Once", fakeHasher);
    // Single use: a forwarded link must not mint a second account.
    expect(
      await status(() =>
        members.acceptInvite(created.token, "another-password-1!", "Twice", fakeHasher),
      ),
    ).not.toBe("ok");
  });

  it("rejects an unknown or tampered token", async () => {
    expect(await status(() => members.readInvite("not-a-real-token"))).toBe(404);
    expect(await status(() => members.readInvite(""))).toBe(404);
  });

  it("refuses to invite someone who is already a member", async () => {
    const inviter = await makeUser();
    await addMember(inviter.id, "admin");
    const existing = await makeUser({ email: "already@acme.dev" });
    await addMember(existing.id, "viewer");
    // 409, the BFF's own conflict: a legitimate request refused by the state.
    expect(
      await status(() => members.createInvite(ORG, "already@acme.dev", "steward", inviter.id)),
    ).toBe(409);
  });

  it("builds an invite link on the configured public base URL", () => {
    expect(members.inviteLink("tok123")).toBe("https://app.test/invite/tok123");
  });
});

describe("password resets", () => {
  it("round-trips: request, then confirm with the token", async () => {
    const user = await makeUser({ email: "reset@acme.dev" });
    await addMember(user.id, "viewer");

    const created = await members.createPasswordReset("reset@acme.dev");
    expect(created?.token).toBeTruthy();
    if (!created) throw new Error("expected a reset to be created");

    expect(
      await status(() =>
        members.confirmPasswordReset(created.token, "brand-new-pass-1!", fakeHasher),
      ),
    ).toBe("ok");
  });

  it("cannot be used twice", async () => {
    const user = await makeUser({ email: "once-reset@acme.dev" });
    await addMember(user.id, "viewer");
    const created = await members.createPasswordReset("once-reset@acme.dev");
    if (!created) throw new Error("expected a reset to be created");

    await members.confirmPasswordReset(created.token, "brand-new-pass-1!", fakeHasher);
    expect(
      await status(() => members.confirmPasswordReset(created.token, "third-pass-1!", fakeHasher)),
    ).not.toBe("ok");
  });

  it("stores only the token's hash", async () => {
    const user = await makeUser({ email: "hash-reset@acme.dev" });
    await addMember(user.id, "viewer");
    const created = await members.createPasswordReset("hash-reset@acme.dev");
    if (!created) throw new Error("expected a reset to be created");

    const database = await db();
    const stored = await database.select().from(schema.passwordResets);
    expect(JSON.stringify(stored)).not.toContain(created.token);
  });

  it("rejects an unknown token", async () => {
    expect(
      await status(() => members.confirmPasswordReset("nope", "whatever-pass-1!", fakeHasher)),
    ).not.toBe("ok");
  });

  it("does not reveal whether an email has an account", async () => {
    // The route answers the same either way; this asserts the layer below does
    // not throw for an unknown address, which is what would leak it.
    const result = await members.createPasswordReset("nobody@nowhere.test");
    expect(result).toBeNull();
  });
});

describe("hashToken", () => {
  it("is deterministic and does not echo the token", () => {
    const hash = members.hashToken("some-token");
    expect(members.hashToken("some-token")).toBe(hash);
    expect(hash).not.toContain("some-token");
    expect(hash).toHaveLength(64); // sha256, hex
  });

  it("separates different tokens", () => {
    expect(members.hashToken("a")).not.toBe(members.hashToken("b"));
  });
});
