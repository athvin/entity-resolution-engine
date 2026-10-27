import { NextResponse } from "next/server";

import { viewerCredentialFor } from "@/lib/bff/credentials";
import { env } from "@/lib/env";
import { verifyWidgetToken } from "@/lib/widget/token";

/** One shape for every failure. A forged, expired, malformed or unknown token
 * all answer this — telling a forger their signature verified but the clock had
 * passed is a free oracle. */
function notFound(): NextResponse {
  return NextResponse.json({ code: "not_found", message: "not found" }, { status: 404 });
}

/**
 * The widget's data (O11).
 *
 * **The only route in the product that serves without a session**, so every
 * line here is about keeping the blast radius at one record.
 *
 * The token names both the org and the record; neither comes from the URL path
 * or a query parameter, so a holder cannot repoint a valid token at another
 * record or another tenant. There is no pagination and no search — the answer
 * is "duplicates of this one record", and anything enumerable would turn a
 * leaked token into a corpus read.
 *
 * The upstream call uses the org's stored VIEWER key via `viewerCredentialFor`,
 * which cannot fall back to the operator token the way `credentialForAccess`
 * can for a super admin.
 */
export async function GET(
  _request: Request,
  { params }: { params: Promise<{ token: string }> },
): Promise<NextResponse> {
  const { token } = await params;
  const verified = verifyWidgetToken(decodeURIComponent(token));
  if (!verified.ok) return notFound();

  const { org, recordKey } = verified.claims;
  try {
    const bearer = await viewerCredentialFor(org);
    const url = new URL(`/v1/orgs/${org}/duplicates`, env().ERSERVER_BASE_URL);
    url.search = new URLSearchParams({ record_key: recordKey, limit: "10" }).toString();
    const response = await fetch(url, {
      headers: { authorization: `Bearer ${bearer}` },
      cache: "no-store",
    });
    if (!response.ok) return notFound();
    const page = (await response.json()) as { items?: unknown[] };
    return NextResponse.json({ record_key: recordKey, items: page.items ?? [] });
  } catch {
    // Never leaks upstream detail: this endpoint is reachable by anyone with a
    // URL, and an error body is a fingerprint of the backend behind it.
    return notFound();
  }
}
