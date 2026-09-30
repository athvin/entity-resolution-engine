import { NextResponse } from "next/server";
import { z } from "zod";

import { writeAudit } from "@/lib/bff/audit";
import { createInvite, inviteLink, listMembers } from "@/lib/bff/members";
import { relayEmail } from "@/lib/bff/relay";
import { BffFailure, requireOrgAccess, respond } from "@/lib/bff/proxy";

export async function GET(
  _request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    // Viewers may see who else is in the workspace; only admins may change it.
    await requireOrgAccess(org, "viewer");
    return NextResponse.json(await listMembers(org));
  });
}

const inviteSchema = z.object({
  email: z.email().max(200),
  role: z.enum(["viewer", "steward", "admin"]),
});

export async function POST(
  request: Request,
  { params }: { params: Promise<{ org: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org } = await params;
    const access = await requireOrgAccess(org, "admin");
    const parsed = inviteSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(422, "validation", "a valid email and role are required");
    }
    const invite = await createInvite(
      org,
      parsed.data.email,
      parsed.data.role,
      access.identity.user.email,
    );
    const link = inviteLink(invite.token);
    // Email is best-effort: the link comes back either way, so an admin can
    // hand it over directly when the relay is not configured.
    const emailed = await relayEmail({
      to: parsed.data.email,
      template: "invite",
      params: {
        org,
        role: parsed.data.role,
        invited_by: access.identity.user.email,
        link,
      },
      org,
    });
    await writeAudit(
      access.identity,
      "member.invite",
      { email: parsed.data.email, role: parsed.data.role, emailed },
      org,
    );
    return NextResponse.json(
      { invite_id: invite.invite_id, expires_at: invite.expires_at, link, emailed },
      { status: 201 },
    );
  });
}
