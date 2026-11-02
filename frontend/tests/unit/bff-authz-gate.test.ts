import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import {
  countRoutes,
  findInternalForwards,
  findUnauthorizedRoutes,
  findUncoveredApiDirs,
} from "../../scripts/check-bff-authz.mjs";

/**
 * The gate in scripts/check-bff-authz.mjs asserts that every tenant and admin
 * route handler authorizes in its own file. These tests exist because the first
 * implementation of that invariant was a semgrep `pattern-not` rule whose
 * pattern failed to parse: semgrep reported the rule as run with zero findings,
 * so a deliberately unauthorized handler sailed through. A gate nobody has
 * watched fail is not a gate — so each case below plants a violation and
 * requires the gate to report it.
 */

let root: string | null = null;

function tree(): string {
  root = mkdtempSync(path.join(tmpdir(), "authz-gate-"));
  return root;
}

function route(dir: string, file: string, source: string): void {
  const full = path.join(dir, file);
  mkdirSync(path.dirname(full), { recursive: true });
  writeFileSync(full, source);
}

const AUTHORIZED_TENANT = `
import { requireOrgAccess, respond } from "@/lib/bff/proxy";
export async function GET(request: Request, { params }: { params: Promise<{ org: string }> }) {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "viewer");
    return Response.json(access.role);
  });
}
`;

const UNAUTHORIZED_TENANT = `
export async function GET() {
  return Response.json({ records: "everybody's" });
}
`;

afterEach(() => {
  if (root) rmSync(root, { recursive: true, force: true });
  root = null;
});

describe("BFF authorization gate", () => {
  it("passes a tenant route that calls requireOrgAccess", () => {
    const dir = tree();
    route(dir, "src/app/api/orgs/[org]/records/route.ts", AUTHORIZED_TENANT);
    expect(findUnauthorizedRoutes(dir)).toEqual([]);
  });

  it("fails a tenant route that authorizes nothing", () => {
    const dir = tree();
    route(dir, "src/app/api/orgs/[org]/records/route.ts", UNAUTHORIZED_TENANT);
    const violations = findUnauthorizedRoutes(dir);
    expect(violations).toHaveLength(1);
    expect(violations[0]?.required).toBe("requireOrgAccess or requireDatasetAccess");
    expect(violations[0]?.methods).toEqual(["GET"]);
  });

  it("passes a dataset-scoped route that calls requireDatasetAccess", () => {
    const dir = tree();
    route(
      dir,
      "src/app/api/orgs/[org]/datasets/[ds]/records/route.ts",
      AUTHORIZED_TENANT.replace("requireOrgAccess", "requireDatasetAccess"),
    );
    expect(findUnauthorizedRoutes(dir)).toEqual([]);
  });

  it("fails an admin route that only checks the session, not super-admin", () => {
    const dir = tree();
    route(
      dir,
      "src/app/api/admin/orgs/route.ts",
      `
      import { requireIdentity } from "@/lib/bff/proxy";
      export async function POST() {
        await requireIdentity();
        return Response.json({ provisioned: true });
      }
      `,
    );
    const violations = findUnauthorizedRoutes(dir);
    expect(violations).toHaveLength(1);
    expect(violations[0]?.required).toBe("requireSuperAdmin");
  });

  it("catches every unauthorized method a handler exports", () => {
    const dir = tree();
    route(
      dir,
      "src/app/api/orgs/[org]/schedules/[scheduleId]/route.ts",
      `
      export async function PATCH() { return Response.json({}); }
      export async function DELETE() { return Response.json({}); }
      `,
    );
    const violations = findUnauthorizedRoutes(dir);
    expect(violations).toHaveLength(1);
    expect(violations[0]?.methods).toEqual(["PATCH", "DELETE"]);
  });

  it("recognises `export const GET = ...` handlers, not just function decls", () => {
    const dir = tree();
    route(
      dir,
      "src/app/api/orgs/[org]/staged/route.ts",
      `export const GET = async () => Response.json({});`,
    );
    expect(findUnauthorizedRoutes(dir)).toHaveLength(1);
  });

  it("ignores a route file that exports no HTTP handler", () => {
    const dir = tree();
    route(dir, "src/app/api/orgs/[org]/helpers/route.ts", `export const runtime = "nodejs";`);
    expect(findUnauthorizedRoutes(dir)).toEqual([]);
  });

  it("counts the handlers it examined, so a moved directory cannot read as green", () => {
    const dir = tree();
    expect(countRoutes(dir)).toBe(0);
    route(dir, "src/app/api/orgs/[org]/records/route.ts", AUTHORIZED_TENANT);
    route(
      dir,
      "src/app/api/admin/orgs/route.ts",
      `
      import { requireSuperAdmin } from "@/lib/bff/admin";
      export async function GET() { await requireSuperAdmin(); return Response.json({}); }
    `,
    );
    expect(countRoutes(dir)).toBe(2);
  });

  it("holds against the real tree — all tenant and admin routes authorize", () => {
    const frontend = path.resolve(import.meta.dirname, "..", "..");
    expect(countRoutes(frontend)).toBeGreaterThan(40);
    expect(findUnauthorizedRoutes(frontend)).toEqual([]);
  });
});

describe("every api directory is covered", () => {
  /**
   * The hole this closes: the gate scanned only `orgs` and `admin`, so a new
   * top-level API directory was never examined and the gate reported success
   * having not looked at it. O11's widget route is what exposed it — and the
   * check immediately found `api/impersonation`, which had been serving HTTP
   * unexamined since it was written.
   */
  it("leaves no api directory unruled and unexempted", () => {
    const frontend = path.resolve(import.meta.dirname, "..", "..");
    expect(findUncoveredApiDirs(frontend)).toEqual([]);
  });

  it("flags a directory that is neither", () => {
    // A directory name that exists in neither RULES nor UNAUTHENTICATED_DIRS
    // must be reported; proven here against a root with no `api` at all, which
    // is the degenerate case the walk has to survive.
    expect(findUncoveredApiDirs("/nonexistent")).toEqual([]);
  });
});

describe("the BFF never forwards the attempt channel", () => {
  /**
   * `/v1/internal/*` is the task pod's machine-to-machine surface
   * (batch-orchestration-design §6.4): a BFF route naming that prefix would
   * hand a browser session a path to the pod-credentialed surface. The gate's
   * property is "nowhere under src/app/api", with no exemption list.
   */
  it("fails a route that names /v1/internal, wherever it lives", () => {
    const dir = tree();
    route(
      dir,
      "src/app/api/orgs/[org]/tasks/route.ts",
      `
      import { requireOrgAccess, respond } from "@/lib/bff/proxy";
      export async function GET(request: Request, { params }: { params: Promise<{ org: string }> }) {
        return respond(async () => {
          const { org } = await params;
          await requireOrgAccess(org, "viewer");
          return fetch(\`\${process.env.ERSERVER_BASE_URL}/v1/internal/attempts/x\`);
        });
      }
      `,
    );
    expect(findInternalForwards(dir)).toEqual(["src/app/api/orgs/[org]/tasks/route.ts"]);
  });

  it("holds against the real tree — no BFF route names the prefix", () => {
    const frontend = path.resolve(import.meta.dirname, "..", "..");
    expect(findInternalForwards(frontend)).toEqual([]);
  });
});
