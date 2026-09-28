import { NextResponse } from "next/server";

import { forward, requireOrgAccess, respond } from "@/lib/bff/proxy";

export async function DELETE(
  _request: Request,
  { params }: { params: Promise<{ org: string; scheduleId: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, scheduleId } = await params;
    const access = await requireOrgAccess(org, "admin");
    const result = await forward<unknown>(
      access,
      `/v1/orgs/${org}/schedules/${encodeURIComponent(scheduleId)}`,
      { method: "DELETE", role: "admin" },
    );
    return NextResponse.json(result);
  });
}

/** Enable/disable (backend batch 2). Body: {enabled: boolean}. */
export async function PATCH(
  request: Request,
  { params }: { params: Promise<{ org: string; scheduleId: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, scheduleId } = await params;
    const access = await requireOrgAccess(org, "admin");
    const body: unknown = await request.json().catch(() => ({}));
    const result = await forward<unknown>(
      access,
      `/v1/orgs/${org}/schedules/${encodeURIComponent(scheduleId)}`,
      { method: "PATCH", role: "admin", body },
    );
    return NextResponse.json(result);
  });
}
