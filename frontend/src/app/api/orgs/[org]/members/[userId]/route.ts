import { NextResponse } from "next/server";
import { z } from "zod";

import { writeAudit } from "@/lib/bff/audit";
import { changeRole, removeMember } from "@/lib/bff/members";
import { BffFailure, requireOrgAccess, respond } from "@/lib/bff/proxy";

const roleSchema = z.object({ role: z.enum(["viewer", "steward", "admin"]) });

export async function PATCH(
  request: Request,
  { params }: { params: Promise<{ org: string; userId: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, userId } = await params;
    const access = await requireOrgAccess(org, "admin");
    const parsed = roleSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(422, "validation", "role must be viewer, steward or admin");
    }
    // Nobody demotes themselves: an admin who did would lose the very power
    // needed to undo it, and the last-admin guard would not catch a workspace
    // with two admins where one locks the other out of their own account.
    if (userId === access.identity.user.id && parsed.data.role !== "admin") {
      throw new BffFailure(
        409,
        "conflict",
        "you cannot change your own role — ask another admin",
      );
    }
    await changeRole(org, userId, parsed.data.role);
    await writeAudit(access.identity, "member.role", { userId, role: parsed.data.role }, org);
    return NextResponse.json({ user_id: userId, role: parsed.data.role });
  });
}

export async function DELETE(
  _request: Request,
  { params }: { params: Promise<{ org: string; userId: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { org, userId } = await params;
    const access = await requireOrgAccess(org, "admin");
    if (userId === access.identity.user.id) {
      throw new BffFailure(409, "conflict", "you cannot remove yourself from the workspace");
    }
    await removeMember(org, userId);
    await writeAudit(access.identity, "member.remove", { userId }, org);
    return NextResponse.json({ removed: true });
  });
}
