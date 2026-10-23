import { NextResponse } from "next/server";

import { listFrom } from "@/lib/bff/envelope";
import { BffFailure, forward, requireOrgAccess, respond } from "@/lib/bff/proxy";
import { parseTagInput } from "@/lib/bff/tags";

/**
 * A subject pair narrows the vocabulary to one thing's chips. Both or neither:
 * half a pair would read as "this thing has every tag", which er-server refuses
 * with a 422 for the same reason.
 */
const PASSTHROUGH = ["subject_type", "subject_id"] as const;

/**
 * The tenant's tag vocabulary, or the live tags on one subject.
 *
 * er-server wraps this in `{tags}`; unwrapped here to the bare array the
 * browser contract uses, with the key checked against
 * `src/lib/api/envelopes.json` by `tests/unit/bff-envelope-contract.test.ts`.
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
    for (const key of PASSTHROUGH) {
      const value = incoming.get(key);
      if (value !== null) searchParams.set(key, value);
    }
    const page = await forward<unknown>(access, `/v1/orgs/${org}/tags`, { searchParams });
    return NextResponse.json(listFrom(page, "tags"));
  });
}

/**
 * Add a label to the vocabulary. Steward+, because naming the work is part of
 * doing it.
 *
 * A duplicate name is er-server's 409 from the partial unique index, surfaced
 * as-is: the BFF cannot pre-check it without racing two stewards naming the
 * same tag.
 */
export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "steward");
    const parsed = parseTagInput(await request.json());
    if (!parsed.ok) {
      throw new BffFailure(422, "validation", parsed.reason);
    }
    const created = await forward<unknown>(access, `/v1/orgs/${org}/tags`, {
      method: "POST",
      // `role` defaults to viewer AND selects the stored key, so omitting it
      // sends a viewer credential to a steward-only endpoint.
      role: "steward",
      body: parsed.value,
    });
    return NextResponse.json(created, { status: 201 });
  });
}
