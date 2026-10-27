import { NextResponse } from "next/server";
import { z } from "zod";

import { BffFailure, requireOrgAccess, respond } from "@/lib/bff/proxy";
import { DEFAULT_TTL_SECONDS, MAX_TTL_SECONDS, mintWidgetToken } from "@/lib/widget/token";

const requestSchema = z.object({
  record_key: z.string().trim().min(1).max(200),
  ttl_seconds: z.number().int().min(60).max(MAX_TTL_SECONDS).optional(),
});

/**
 * Mint a widget token for one record (O11).
 *
 * **Admin, deliberately.** Minting a token creates a credential that works
 * without a session, in a page we do not control. That is an administrative
 * act even though what it unlocks is read-only — the person embedding the
 * widget is deciding where this tenant's data gets rendered.
 *
 * One token per RECORD. A token scoped to the org would, if it leaked from a
 * single embed, read the whole corpus.
 */
export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    await requireOrgAccess(org, "admin");
    const parsed = requestSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(422, "validation", "record_key is required");
    }
    const ttl = parsed.data.ttl_seconds ?? DEFAULT_TTL_SECONDS;
    return NextResponse.json({
      token: mintWidgetToken(org, parsed.data.record_key, ttl),
      expires_in: ttl,
    });
  });
}
