import { NextResponse } from "next/server";

import { serviceKeyIds } from "@/lib/bff/credentials";
import { BffFailure, forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

export async function DELETE(
  _request: Request,
  { params }: { params: Promise<{ org: string; keyId: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, keyId } = await params;
    const access = await requireOrgAccess(org, "admin");
    // The app calls er-server with these keys. Revoking one would sign the app
    // out of this org for every member, so it is refused here rather than
    // left to be discovered as an outage.
    if ((await serviceKeyIds(org)).has(keyId)) {
      throw new BffFailure(
        409,
        "conflict",
        "this key is the web app's own credential for this organization; revoking it would sign the app out",
      );
    }
    // Idempotent on er-server: revoking an already-revoked key answers
    // `{revoked: false}` rather than an error.
    const result = await forward<unknown>(
      access,
      `/v1/orgs/${org}/api-keys/${encodeURIComponent(keyId)}`,
      { method: "DELETE", role: "admin" },
    );
    return NextResponse.json(result);
  });
}
