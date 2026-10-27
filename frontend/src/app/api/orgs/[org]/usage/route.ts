import { NextResponse } from "next/server";

import { BffFailure, requireOrgAccess, respond } from "@/lib/bff/proxy";
import { usageSince } from "@/lib/usage/record";

/** How far back a usage read may look. A year is the longest window anyone
 * reasons about, and an unbounded one is a full-table scan a URL can trigger. */
const MAX_DAYS = 366;
const DEFAULT_DAYS = 30;

/**
 * What this org has consumed (O12b).
 *
 * **Any member may read it.** Usage is not sensitive the way a credential is,
 * and the person who most needs to see "the assistant is burning tokens" is
 * often a steward who cannot change the AI settings — telling them to ask an
 * admin for a number is how a surprising bill becomes a surprising argument.
 *
 * Metering, not billing: no price appears here, because the pricing story is
 * per-run on cheap compute and a figure nobody can reconcile against an invoice
 * is worse than no figure at all.
 */
export async function GET(
  request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    await requireOrgAccess(org, "viewer");

    const raw = new URL(request.url).searchParams.get("days");
    const days = raw === null ? DEFAULT_DAYS : Number(raw);
    if (!Number.isInteger(days) || days < 1 || days > MAX_DAYS) {
      throw new BffFailure(
        422,
        "validation",
        `days must be an integer from 1 to ${String(MAX_DAYS)}`,
      );
    }

    const since = new Date(Date.now() - days * 24 * 60 * 60 * 1000);
    return NextResponse.json({
      days,
      since: since.toISOString(),
      rows: await usageSince(org, since),
    });
  });
}
