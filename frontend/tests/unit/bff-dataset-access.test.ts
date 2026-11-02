import { beforeEach, describe, expect, it, vi } from "vitest";

import type { Identity } from "@/lib/auth/session";
import type { Membership, OrgRole } from "@/lib/auth/types";

/**
 * The dataset half of the BFF access model (datasets program, PR2 — inert
 * until the dataset routes land).
 *
 * `requireDatasetAccess` must be exactly `requireOrgAccess` plus the dataset
 * slug check: authorization stays org-membership, a dataset only narrows the
 * scope. The interesting properties are the ones that would rot silently —
 * that a traversal-shaped dataset name 404s before any URL is built (the
 * segment is raw-interpolated with the BFF's privileged credential attached,
 * exactly like `org`), and that the org checks still run first so the denial
 * statuses are unchanged.
 */

const identityFromCookies = vi.hoisted(() => vi.fn<() => Promise<Identity | null>>());

vi.mock("@/lib/auth/session", () => ({ identityFromCookies }));

const { BffFailure, requireDatasetAccess, requireDatasetSlug, datasetPath } =
  await import("@/lib/bff/proxy");

const ORG = "acme-dev";

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

async function attempt(
  org: string,
  dataset: string,
  minimum: OrgRole,
): Promise<number | "allowed"> {
  try {
    await requireDatasetAccess(org, dataset, minimum);
    return "allowed";
  } catch (error) {
    if (error instanceof BffFailure) return error.status;
    throw error;
  }
}

beforeEach(() => {
  identityFromCookies.mockReset();
});

describe("requireDatasetSlug", () => {
  it.for(["default", "crm-contacts", "accounts_2026", "a", "0ds"] as const)("accepts %s", (ds) => {
    expect(() => {
      requireDatasetSlug(ds);
    }).not.toThrow();
  });

  it.for([
    "..",
    "../",
    "../../v1/orgs/victim",
    "default/../victim",
    "./default",
    "..%2f..%2fvictim",
    "Default", // uppercase is not in the mint grammar
    "-leading-dash",
    "",
  ] as const)("rejects %s with 404", (ds) => {
    try {
      requireDatasetSlug(ds);
      throw new Error("expected requireDatasetSlug to throw");
    } catch (error) {
      if (!(error instanceof BffFailure)) throw error;
      // 404, not 422: a malformed probe learns no more than an unauthorized one.
      expect(error.status).toBe(404);
    }
  });

  it("rejects a name longer than the mint maximum", () => {
    expect(() => {
      requireDatasetSlug("a".repeat(129));
    }).toThrow(BffFailure);
    expect(() => {
      requireDatasetSlug("a".repeat(128));
    }).not.toThrow();
  });
});

describe("requireDatasetAccess", () => {
  it("is org access plus the dataset scope", async () => {
    identityFromCookies.mockResolvedValue(identity({ memberships: [member(ORG, "steward")] }));
    const access = await requireDatasetAccess(ORG, "crm", "steward");
    expect(access).toMatchObject({ org: ORG, dataset: "crm", role: "steward" });
  });

  it("keeps the org denial statuses: 401 no session, 403 under-ranked, 404 non-member", async () => {
    identityFromCookies.mockResolvedValue(null);
    await expect(attempt(ORG, "crm", "viewer")).resolves.toBe(401);

    identityFromCookies.mockResolvedValue(identity({ memberships: [member(ORG, "viewer")] }));
    await expect(attempt(ORG, "crm", "admin")).resolves.toBe(403);

    identityFromCookies.mockResolvedValue(identity({ memberships: [] }));
    await expect(attempt(ORG, "crm", "viewer")).resolves.toBe(404);
  });

  it("404s a traversal-shaped dataset even for a member", async () => {
    identityFromCookies.mockResolvedValue(identity({ memberships: [member(ORG, "admin")] }));
    await expect(attempt(ORG, "../victim", "viewer")).resolves.toBe(404);
  });
});

describe("datasetPath", () => {
  it("builds the dataset-scoped upstream path from the access value", async () => {
    identityFromCookies.mockResolvedValue(identity({ memberships: [member(ORG, "viewer")] }));
    const access = await requireDatasetAccess(ORG, "crm", "viewer");
    expect(datasetPath(access, "/golden-records")).toBe(
      "/v1/orgs/acme-dev/datasets/crm/golden-records",
    );
    expect(datasetPath(access, "")).toBe("/v1/orgs/acme-dev/datasets/crm");
  });
});
