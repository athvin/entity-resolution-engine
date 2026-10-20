import { describe, expect, it } from "vitest";

import { BffFailure, ORG_SLUG, requireOrgSlug } from "@/lib/bff/proxy";

/**
 * `org` was the one path parameter never `encodeURIComponent`-ed: ~46 tenant
 * routes and 4 admin routes interpolate it straight into `/v1/orgs/${org}/…`,
 * and `new URL()` resolves `..` segments — so a name containing them re-points
 * the upstream request, with the BFF's privileged credential attached.
 *
 * Membership lookup already made that unreachable for tenant users. The gap was
 * the super-admin branch (which accepts any string) and the admin routes (which
 * never call requireOrgAccess at all). A super admin can already reach every
 * org, so this is not privilege escalation — it is removing a traversal
 * primitive and keeping the audit trail honest.
 */

/** Pull the status off the thrown BffFailure, or fail the test. */
function statusOf(run: () => void): number {
  try {
    run();
  } catch (error) {
    if (error instanceof BffFailure) return error.status;
    throw error;
  }
  throw new Error("expected requireOrgSlug to throw");
}

describe("requireOrgSlug", () => {
  describe("accepts the slugs erserver actually mints", () => {
    it.for([
      "acme-dev",
      "acme",
      "mutable-dev",
      "wizard-android",
      "selectall-dev",
      "org_with_underscores",
      "a",
      "0rg",
      "tenant123",
    ] as const)("%s", (org) => {
      expect(() => {
        requireOrgSlug(org);
      }).not.toThrow();
    });
  });

  describe("rejects path traversal", () => {
    it.for([
      "..",
      "../",
      "../..",
      "../../v1/orgs/victim",
      "acme/../victim",
      "./acme",
      // Pre-encoded forms: Next decodes the route param, so these arrive as
      // literal separators by the time the BFF sees them.
      "..%2f..%2fvictim",
      "acme%2f..%2fvictim",
    ] as const)("%s", (org) => {
      expect(() => {
        requireOrgSlug(org);
      }).toThrow();
    });
  });

  describe("rejects anything that is not a slug", () => {
    it.for([
      "", // empty
      "/acme", // leading separator
      "acme/", // trailing separator
      "acme dev", // space
      "Acme-Dev", // uppercase: erserver mints lowercase only
      "acme.dev", // dot
      "acme:dev", // the lifecycle route builds `${org}:${action}`
      "acme?x=1", // query injection into the upstream URL
      "acme#frag",
      "acme&x=1",
      "-acme", // must start alphanumeric
      "_acme",
      "http://evil.example",
      "//evil.example",
    ] as const)("%s", (org) => {
      expect(() => {
        requireOrgSlug(org);
      }).toThrow();
    });
  });

  it("rejects control characters that could split an upstream header", () => {
    const CR = String.fromCharCode(0x0d);
    const LF = String.fromCharCode(0x0a);
    const NUL = String.fromCharCode(0x00);
    expect(() => {
      requireOrgSlug(`acme${CR}${LF}x`);
    }).toThrow();
    expect(() => {
      requireOrgSlug(`acme${NUL}`);
    }).toThrow();
  });

  it("rejects a name longer than the provisioning schema allows", () => {
    expect(() => {
      requireOrgSlug("a".repeat(128));
    }).not.toThrow();
    expect(() => {
      requireOrgSlug("a".repeat(129));
    }).toThrow();
  });

  it("answers 404, the same as a non-member, so a probe learns nothing extra", () => {
    // Deliberately not 422: a malformed name and an org you may not see must be
    // indistinguishable, or the error code becomes an existence oracle.
    expect(
      statusOf(() => {
        requireOrgSlug("../victim");
      }),
    ).toBe(404);
    expect(
      statusOf(() => {
        requireOrgSlug("Acme");
      }),
    ).toBe(404);
  });

  it("shares one definition with the org-provisioning schema", () => {
    // api/admin/orgs validates new names with this same shape; if the two drift,
    // a name could be mintable but unreadable.
    expect(ORG_SLUG.source).toBe("^[a-z0-9][a-z0-9_-]*$");
  });
});
