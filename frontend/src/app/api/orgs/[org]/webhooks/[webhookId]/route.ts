import { NextResponse } from "next/server";

import { forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

export async function DELETE(
  _request: Request,
  { params }: { params: Promise<{ org: string; webhookId: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, webhookId } = await params;
    const access = await requireOrgAccess(org, "admin");
    const result = await forward<unknown>(
      access,
      `/v1/orgs/${org}/webhooks/${encodeURIComponent(webhookId)}`,
      { method: "DELETE", role: "admin" },
    );
    return NextResponse.json(result);
  });
}
