import { NextResponse } from "next/server";

import { listFrom } from "@/lib/bff/envelope";
import { parseLabelInput } from "@/lib/bff/labels";
import { BffFailure, forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

/**
 * Recording a verdict on a pair, and reading the verdicts recorded.
 *
 * **The POST must carry `model_version` and `score_at_label` from the queue
 * item it answers.** Both are nullable on the entity, and a label recorded
 * without them is still a perfectly good training example — but it is invisible
 * to per-version evaluation, which can only use labels carrying the score that
 * version actually produced. Dropping them here would leave
 * `/models/{name}/versions/{v}/evaluation` reporting "no label can be scored"
 * forever, with nothing anywhere to explain why. The client reads both off the
 * queue item rather than inventing them; the queue returns `model_version` per
 * row for exactly this.
 */
export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "steward");
    const parsed = parseLabelInput(await request.json());
    if (!parsed.ok) {
      throw new BffFailure(422, "validation", parsed.reason);
    }
    const label = await forward<unknown>(access, `/v1/orgs/${org}/labels`, {
      method: "POST",
      // `role` defaults to VIEWER and selects the stored key, so omitting it
      // forwards a viewer credential to a steward-only endpoint and gets a 403
      // that looks like a permissions bug in the product.
      role: "steward",
      // An object, not a JSON string: `forward` serializes `body` itself, so
      // stringifying first sends a JSON string *literal* and er-server cannot
      // deserialize it into `LabelIn`.
      body: parsed.value,
    });
    return NextResponse.json(label, { status: 201 });
  });
}

/**
 * The verdicts recorded so far, newest first.
 *
 * er-server wraps this one in `{labels}` — unwrapped here to the bare array the
 * browser contract uses. The key is checked against
 * `src/lib/api/envelopes.json` by `tests/unit/bff-envelope-contract.test.ts`,
 * because `listFrom` answers `[]` for a wrong key and an empty table is not an
 * error anyone would notice.
 */
export async function GET(
  request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "viewer");
    const incoming = new URL(request.url).searchParams;
    const searchParams = new URLSearchParams();
    for (const key of ["verdict", "origin", "limit"] as const) {
      const value = incoming.get(key);
      if (value !== null) searchParams.set(key, value);
    }
    const page = await forward<unknown>(access, `/v1/orgs/${org}/labels`, { searchParams });
    return NextResponse.json(listFrom(page, "labels"));
  });
}
