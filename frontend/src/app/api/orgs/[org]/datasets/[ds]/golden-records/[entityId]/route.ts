import { NextResponse } from "next/server";

import { BffFailure, forward, requireDatasetAccess, respond } from "@/lib/bff/proxy";

/**
 * One resolved entity: golden values, members, lineage and events.
 *
 * `?snapshot=N` (O12c) reads the entity AS IT WAS at lake version N, which is
 * how the field-history panel shows what a value used to be. DuckLake keeps the
 * old versions, so the history needs no new table — only the ability to ask for
 * a version other than the newest.
 *
 * Validated here rather than passed through: a non-numeric snapshot would reach
 * the lake as a SQL-level type error and surface as a 503 that reads like an
 * outage.
 */
export async function GET(
  request: Request,
  { params }: { params: Promise<{ org: string; ds: string; entityId: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, ds, entityId } = await params;
    const access = await requireDatasetAccess(org, ds, "viewer");

    const searchParams = new URLSearchParams();
    const raw = new URL(request.url).searchParams.get("snapshot");
    if (raw !== null) {
      const snapshot = Number(raw);
      if (!Number.isInteger(snapshot) || snapshot < 0) {
        throw new BffFailure(422, "validation", "snapshot must be a non-negative integer");
      }
      searchParams.set("snapshot", String(snapshot));
    }

    const detail = await forward<unknown>(
      access,
      `/v1/orgs/${org}/datasets/${ds}/golden-records/${encodeURIComponent(entityId)}`,
      searchParams.size > 0 ? { searchParams } : {},
    );
    return NextResponse.json(detail);
  });
}
