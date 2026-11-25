/**
 * #896: `forwardUnlessNotImplemented` turns an upstream 501 — and only a 501 —
 * into `null`.
 *
 * The segment export prequeries tags for an optional column. A Postgres-less
 * er-server answers those reads `501 tags require Postgres`, and before this
 * the BFF mapped any upstream 5xx to a blanket 502, so the whole export failed
 * on a deploy shape where the column could never be non-empty. The fix is
 * deliberately narrow: the server's 501 stays loud for every other caller, and
 * every other status still fails exactly as `forward` fails.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { EffectiveOrgAccess } from "@/lib/bff/proxy";

vi.mock("@/lib/auth/session", () => ({ identityFromCookies: vi.fn() }));

const { BffFailure, forward, forwardUnlessNotImplemented } = await import("@/lib/bff/proxy");

const ENV = {
  ERSERVER_BASE_URL: "http://erserver.test",
  ERSERVER_OPERATOR_TOKEN: "op-token",
  ERWEB_DATABASE_URL: "pglite://memory",
  ERWEB_SESSION_SECRET: "x".repeat(32),
  ERWEB_CREDENTIAL_KEY: Buffer.alloc(32, 7).toString("base64"),
};

// Operator-routed, so the credential comes from the env and no database is read.
const ACCESS = {
  identity: { user: { email: "op@example.test" }, impersonating: false },
  org: "acme",
  role: "admin",
  viaOperator: true,
} as unknown as EffectiveOrgAccess;

function answer(status: number, body: unknown) {
  vi.stubGlobal(
    "fetch",
    vi.fn(() =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          status,
          headers: { "content-type": "application/json" },
        }),
      ),
    ),
  );
}

describe("forwardUnlessNotImplemented (#896)", () => {
  beforeEach(() => {
    for (const [k, v] of Object.entries(ENV)) vi.stubEnv(k, v);
  });
  afterEach(() => {
    vi.unstubAllEnvs();
    vi.unstubAllGlobals();
  });

  it("returns the body when the read succeeds", async () => {
    answer(200, { tags: [{ id: "t1" }] });
    await expect(forwardUnlessNotImplemented(ACCESS, "/v1/orgs/acme/tags")).resolves.toEqual({
      tags: [{ id: "t1" }],
    });
  });

  it("is null for an upstream 501, where forward still fails", async () => {
    answer(501, { detail: "tags require Postgres" });
    await expect(forwardUnlessNotImplemented(ACCESS, "/v1/orgs/acme/tags")).resolves.toBeNull();
    // The plain `forward` keeps the 501 loud: a lost DSN must not read as
    // "no tags" anywhere the value is required.
    await expect(forward(ACCESS, "/v1/orgs/acme/tags")).rejects.toBeInstanceOf(BffFailure);
  });

  it("still fails on every other status", async () => {
    for (const status of [503, 500, 404, 403]) {
      answer(status, { detail: "no" });
      await expect(
        forwardUnlessNotImplemented(ACCESS, "/v1/orgs/acme/tags"),
        `status ${String(status)}`,
      ).rejects.toBeInstanceOf(BffFailure);
    }
  });
});
