import { NextResponse } from "next/server";

import { BffFailure, forward, requireOrgAccess, respond } from "@/lib/bff/proxy";
import { isSameOrg, parseCompare, parseImport } from "@/lib/bff/promote";
import { diffLines } from "@/lib/domain/config";

interface ConfigPayload {
  yaml: string;
  version?: number;
}

/**
 * Compare this org's published config against another org's (O9).
 *
 * **Both orgs are authorized, at admin, independently.** The obvious mistake is
 * to check only the org in the URL — which would let someone who administers
 * staging read production's configuration through this endpoint. A config is
 * not secret in the way a credential is, but it names sources, thresholds and
 * business rules, and reading it is a privilege of administering the tenant it
 * belongs to.
 *
 * `POST` rather than `GET` because the other org arrives in a body: putting an
 * org name in a query string makes it a referrer-logged, cache-keyed value, and
 * this one is an authorization input.
 */
export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const parsed = parseCompare(await request.json().catch(() => null));
    if (!parsed.ok) {
      throw new BffFailure(422, "validation", parsed.reason);
    }
    const { against, version } = parsed.value;
    if (isSameOrg(org, against)) {
      throw new BffFailure(422, "validation", "comparing a config against itself says nothing");
    }

    // TWO checks, both admin. Order is deliberate: the URL's org first, so a
    // caller with no access to it gets the same 404 every other route gives
    // and learns nothing about whether `against` exists.
    const mine = await requireOrgAccess(org, "admin");
    const theirs = await requireOrgAccess(against, "admin");

    const [here, there] = await Promise.all([
      forward<ConfigPayload>(mine, `/v1/orgs/${org}/config`),
      forward<ConfigPayload>(
        theirs,
        version === undefined
          ? `/v1/orgs/${against}/config`
          : `/v1/orgs/${against}/config/versions/${String(version)}`,
      ),
    ]);

    return NextResponse.json({
      org,
      against,
      // Diffed server-side with the SAME function the version history uses, so
      // a cross-org diff and an in-org one cannot disagree about what changed.
      diff: diffLines(here.yaml, there.yaml),
      their_version: there.version ?? null,
      my_version: here.version ?? null,
    });
  });
}

/**
 * Copy another org's config in, as a DRAFT (O9's import half).
 *
 * Never publishes — the same rule every write in this program follows. The diff
 * lands in the config studio's version history and an admin decides. That
 * matters more here than for an AI draft: a promoted config comes from a
 * different tenant and may name sources this one does not have, which
 * `er-sql-check` refuses at publish rather than at import.
 */
export async function PUT(
  request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const parsed = parseImport(await request.json().catch(() => null));
    if (!parsed.ok) {
      throw new BffFailure(422, "validation", parsed.reason);
    }
    const { from, version } = parsed.value;
    if (isSameOrg(org, from)) {
      throw new BffFailure(
        422,
        "validation",
        "importing an org's config into itself would stage a draft nobody meant to make",
      );
    }

    const mine = await requireOrgAccess(org, "admin");
    const theirs = await requireOrgAccess(from, "admin");

    const source = await forward<ConfigPayload>(
      theirs,
      version === undefined
        ? `/v1/orgs/${from}/config`
        : `/v1/orgs/${from}/config/versions/${String(version)}`,
    );
    if (typeof source.yaml !== "string" || source.yaml.trim() === "") {
      throw new BffFailure(422, "validation", `${from} has no published configuration to copy`);
    }

    const created = await forward<unknown>(mine, `/v1/orgs/${org}/config/versions`, {
      method: "POST",
      role: "admin",
      body: { yaml: source.yaml },
    });
    return NextResponse.json(
      { imported_from: from, imported_version: source.version ?? null, version: created },
      { status: 201 },
    );
  });
}
