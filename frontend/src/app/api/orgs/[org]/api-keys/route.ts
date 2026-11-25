import { NextResponse } from "next/server";
import { z } from "zod";

import { serviceKeyIds } from "@/lib/bff/credentials";
import { listFrom } from "@/lib/bff/envelope";
import { BffFailure, forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

/**
 * The org's API keys: metadata only.
 *
 * er-server lists through `ApiKeyWire`, which has no `key_hash` field, and the
 * plaintext exists only in the mint response — so there is nothing secret here
 * to filter, and this route does not try to. Admin-only on both sides.
 */
export async function GET(
  _request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "admin");
    // `{keys: [...]}` on the wire (the envelope ledger's `wrapped`/`keys`), a
    // bare array to the browser.
    const body = await forward<unknown>(access, `/v1/orgs/${org}/api-keys`, { role: "admin" });
    // Mark the app's own service keys so the screen can say what they are and
    // not offer to revoke them (the DELETE route refuses them too).
    const service = await serviceKeyIds(org);
    const keys = listFrom<{ id: string }>(body, "keys").map((key) => ({
      ...key,
      service: service.has(key.id),
    }));
    return NextResponse.json(keys);
  });
}

const createSchema = z.object({
  // `operator` is not a mintable role and er-server refuses it (422); not
  // offering it here saves the round trip, but the server stays the authority.
  role: z.enum(["viewer", "steward", "admin"]),
});

export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "admin");
    const parsed = createSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(422, "validation", "role must be viewer, steward or admin");
    }
    const created = await forward<unknown>(access, `/v1/orgs/${org}/api-keys`, {
      method: "POST",
      role: "admin",
      body: parsed.data,
    });
    // Passed through whole: the response carries the plaintext `key`, and this
    // is the only time it is ever returned — the list never shows it again.
    return NextResponse.json(created, { status: 201 });
  });
}
