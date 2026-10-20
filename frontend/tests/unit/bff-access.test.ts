import { beforeEach, describe, expect, it, vi } from "vitest";

// Identity is declared in auth/session (which is mocked below); Membership and
// OrgRole in auth/types. Type-only imports are erased, so importing from the
// mocked module is safe.
import type { Identity } from "@/lib/auth/session";
import type { Membership, OrgRole } from "@/lib/auth/types";

/**
 * `requireOrgAccess` is the whole tenant-isolation, RBAC and impersonation model
 * of this application, in one function — and it had no unit test at all.
 *
 * The Playwright tier covers the happy paths through real screens, but it cannot
 * enumerate this matrix: three roles x three required minimums, crossed with
 * membership, super-admin-without-membership, and active impersonation. That is
 * what this file does, because the interesting cases are the denials, and the
 * most interesting property is *which* denial — 404 where a 403 would leak that
 * an org exists.
 *
 * `identityFromCookies` is mocked rather than stubbed through the database: the
 * question here is purely "given this identity, what may it do", and the session
 * resolution it would exercise is covered separately in session.test.ts.
 */

const identityFromCookies = vi.hoisted(() => vi.fn<() => Promise<Identity | null>>());

vi.mock("@/lib/auth/session", () => ({ identityFromCookies }));

const { BffFailure, requireOrgAccess, requireIdentity } = await import("@/lib/bff/proxy");

const ORG = "acme-dev";
const OTHER_ORG = "globex-dev";
const ROLES: readonly OrgRole[] = ["viewer", "steward", "admin"];

function identity(overrides: Partial<Identity> = {}): Identity {
  return {
    sessionId: "01JBQSESSION",
    user: {
      id: "01JBQUSER",
      email: "admin@acme.dev",
      displayName: "Ada",
      isSuperAdmin: false,
    },
    memberships: [],
    impersonating: null,
    ...overrides,
  };
}

function member(org: string, role: OrgRole): Membership {
  return { org, role };
}

/** Run requireOrgAccess and report the HTTP status it denied with, or "allowed". */
async function attempt(org: string, minimum: OrgRole): Promise<number | "allowed"> {
  try {
    await requireOrgAccess(org, minimum);
    return "allowed";
  } catch (error) {
    if (error instanceof BffFailure) return error.status;
    throw error;
  }
}

beforeEach(() => {
  identityFromCookies.mockReset();
});

describe("requireIdentity", () => {
  it("401s when there is no session", async () => {
    identityFromCookies.mockResolvedValue(null);
    await expect(requireIdentity()).rejects.toBeInstanceOf(BffFailure);
    identityFromCookies.mockResolvedValue(null);
    await expect(attempt(ORG, "viewer")).resolves.toBe(401);
  });

  it("returns the identity when there is one", async () => {
    const who = identity({ memberships: [member(ORG, "admin")] });
    identityFromCookies.mockResolvedValue(who);
    await expect(requireIdentity()).resolves.toBe(who);
  });
});

describe("requireOrgAccess — the role rank", () => {
  // viewer < steward < admin. A member meets a minimum iff their rank is >= it.
  const expected: Record<OrgRole, Record<OrgRole, number | "allowed">> = {
    viewer: { viewer: "allowed", steward: 403, admin: 403 },
    steward: { viewer: "allowed", steward: "allowed", admin: 403 },
    admin: { viewer: "allowed", steward: "allowed", admin: "allowed" },
  };

  for (const held of ROLES) {
    for (const minimum of ROLES) {
      it(`a ${held} meeting a ${minimum} minimum → ${String(expected[held][minimum])}`, async () => {
        identityFromCookies.mockResolvedValue(identity({ memberships: [member(ORG, held)] }));
        expect(await attempt(ORG, minimum)).toBe(expected[held][minimum]);
      });
    }
  }

  it("reports the role actually held, not the minimum asked for", async () => {
    identityFromCookies.mockResolvedValue(identity({ memberships: [member(ORG, "admin")] }));
    const access = await requireOrgAccess(ORG, "viewer");
    expect(access.role).toBe("admin");
    expect(access.org).toBe(ORG);
    expect(access.viaOperator).toBe(false);
  });

  it("picks the membership for the requested org, not the first one held", async () => {
    identityFromCookies.mockResolvedValue(
      identity({ memberships: [member(OTHER_ORG, "admin"), member(ORG, "viewer")] }),
    );
    const access = await requireOrgAccess(ORG, "viewer");
    expect(access.role).toBe("viewer");
    // The admin membership elsewhere must not grant anything here.
    expect(await attempt(ORG, "admin")).toBe(403);
  });
});

describe("requireOrgAccess — tenant isolation", () => {
  it("404s a non-member, so they cannot learn the org exists", async () => {
    identityFromCookies.mockResolvedValue(identity({ memberships: [member(ORG, "admin")] }));
    expect(await attempt(OTHER_ORG, "viewer")).toBe(404);
  });

  it("404s rather than 403s — the status must not be an existence oracle", async () => {
    identityFromCookies.mockResolvedValue(identity({ memberships: [member(ORG, "admin")] }));
    // An org that exists but is not theirs, and one that does not exist at all,
    // must be indistinguishable. Both are 404 because this function cannot tell
    // them apart either — it never asks erserver.
    expect(await attempt(OTHER_ORG, "viewer")).toBe(404);
    expect(await attempt("never-provisioned", "viewer")).toBe(404);
  });

  it("404s a user with no memberships at all", async () => {
    identityFromCookies.mockResolvedValue(identity());
    expect(await attempt(ORG, "viewer")).toBe(404);
  });

  it("distinguishes 403 from 404 correctly: membership decides which", async () => {
    // A member who lacks the ROLE gets 403 — hiding the org from someone who can
    // already see it would be the wrong answer, and tells them nothing new.
    identityFromCookies.mockResolvedValue(identity({ memberships: [member(ORG, "viewer")] }));
    expect(await attempt(ORG, "admin")).toBe(403);
    // A non-member gets 404 for the same request.
    identityFromCookies.mockResolvedValue(identity({ memberships: [member(OTHER_ORG, "admin")] }));
    expect(await attempt(ORG, "admin")).toBe(404);
  });
});

describe("requireOrgAccess — the super admin without a membership", () => {
  const superAdmin = (overrides: Partial<Identity> = {}): Identity =>
    identity({
      user: {
        id: "01JBQROOT",
        email: "root@er.dev",
        displayName: "Root",
        isSuperAdmin: true,
      },
      ...overrides,
    });

  it("is granted admin on any org, via the operator token", async () => {
    identityFromCookies.mockResolvedValue(superAdmin());
    const access = await requireOrgAccess(OTHER_ORG, "admin");
    expect(access.role).toBe("admin");
    // viaOperator is what selects the operator token over a stored org key in
    // credentialForAccess — so this flag decides which credential goes upstream.
    expect(access.viaOperator).toBe(true);
  });

  it("meets every minimum", async () => {
    for (const minimum of ROLES) {
      identityFromCookies.mockResolvedValue(superAdmin());
      expect(await attempt(OTHER_ORG, minimum)).toBe("allowed");
    }
  });

  it("uses a real membership in preference to the operator token", async () => {
    // A super admin who IS a member of this org acts as that member: the stored
    // org key is the narrower credential, so it is the right one to use.
    identityFromCookies.mockResolvedValue(superAdmin({ memberships: [member(ORG, "steward")] }));
    const access = await requireOrgAccess(ORG, "viewer");
    expect(access.role).toBe("steward");
    expect(access.viaOperator).toBe(false);
  });

  it("is still bound by that membership's role where one exists", async () => {
    identityFromCookies.mockResolvedValue(superAdmin({ memberships: [member(ORG, "viewer")] }));
    // Being a super admin does not lift a membership they chose to hold.
    expect(await attempt(ORG, "admin")).toBe(403);
  });
});

describe("requireOrgAccess — impersonation replaces super-admin powers", () => {
  const future = new Date(Date.now() + 60 * 60 * 1000);

  const impersonating = (org: string, role: OrgRole): Identity =>
    identity({
      user: {
        id: "01JBQROOT",
        email: "root@er.dev",
        displayName: "Root",
        isSuperAdmin: true,
      },
      impersonating: { org, role, expiresAt: future },
    });

  it("confines the session to the impersonated org", async () => {
    identityFromCookies.mockResolvedValue(impersonating(ORG, "admin"));
    expect(await attempt(ORG, "admin")).toBe("allowed");
  });

  it("404s every OTHER org — 'view as tenant' means seeing exactly what they see", async () => {
    identityFromCookies.mockResolvedValue(impersonating(ORG, "admin"));
    // Without the impersonation branch this would succeed via the super-admin
    // fallback, which is precisely the bug the branch exists to prevent.
    expect(await attempt(OTHER_ORG, "viewer")).toBe(404);
  });

  it("enforces the impersonated role, not the super admin's reach", async () => {
    identityFromCookies.mockResolvedValue(impersonating(ORG, "viewer"));
    expect(await attempt(ORG, "viewer")).toBe("allowed");
    expect(await attempt(ORG, "steward")).toBe(403);
    expect(await attempt(ORG, "admin")).toBe(403);
  });

  it("acts through the org's own key, not the operator token", async () => {
    identityFromCookies.mockResolvedValue(impersonating(ORG, "steward"));
    const access = await requireOrgAccess(ORG, "steward");
    expect(access.role).toBe("steward");
    // viaOperator false: the whole point is that the request is indistinguishable
    // from the tenant's own, including the credential erserver sees.
    expect(access.viaOperator).toBe(false);
  });

  it("enforces the impersonated role across the full matrix", async () => {
    const expected: Record<OrgRole, Record<OrgRole, number | "allowed">> = {
      viewer: { viewer: "allowed", steward: 403, admin: 403 },
      steward: { viewer: "allowed", steward: "allowed", admin: 403 },
      admin: { viewer: "allowed", steward: "allowed", admin: "allowed" },
    };
    for (const as of ROLES) {
      for (const minimum of ROLES) {
        identityFromCookies.mockResolvedValue(impersonating(ORG, as));
        expect(await attempt(ORG, minimum), `as ${as}, minimum ${minimum}`).toBe(
          expected[as][minimum],
        );
      }
    }
  });

  it("is ignored for a non-super-admin, who falls through to their memberships", async () => {
    // The branch is gated on isSuperAdmin as well as the impersonation record, so
    // a stale record on an ordinary user cannot grant anything.
    identityFromCookies.mockResolvedValue(
      identity({
        memberships: [member(ORG, "viewer")],
        impersonating: { org: ORG, role: "admin", expiresAt: future },
      }),
    );
    expect(await attempt(ORG, "admin")).toBe(403);
    expect(await attempt(ORG, "viewer")).toBe("allowed");
  });
});

describe("requireOrgAccess — the org slug guard runs first", () => {
  it("404s a traversal-shaped name even for a super admin", async () => {
    identityFromCookies.mockResolvedValue(
      identity({
        user: { id: "r", email: "root@er.dev", displayName: "Root", isSuperAdmin: true },
      }),
    );
    // The super-admin branch accepts any org string, so this is the identity for
    // which the slug guard is the only thing standing in the way.
    expect(await attempt("../../v1/orgs/victim", "viewer")).toBe(404);
    expect(await attempt("acme%2f..%2fvictim", "viewer")).toBe(404);
  });

  it("still 401s an absent session before judging the name", async () => {
    identityFromCookies.mockResolvedValue(null);
    // Identity first: a malformed name must not mask an expired session.
    expect(await attempt("../victim", "viewer")).toBe(401);
  });
});
