import { eq, ne, and } from "drizzle-orm";
import { NextResponse } from "next/server";
import { z } from "zod";

import { hashPassword, verifyPassword } from "@/lib/auth/password";
import { writeAudit } from "@/lib/bff/audit";
import { BffFailure, requireIdentity, respond } from "@/lib/bff/proxy";
import { db, schema } from "@/lib/db";

const changeSchema = z.object({
  current_password: z.string().min(1),
  new_password: z.string().min(12).max(200),
});

/** Change your own password. The current one is required — a stolen session
 * must not be enough to take the account over. */
export async function POST(request: Request): Promise<NextResponse> {
  return respond(async () => {
    const identity = await requireIdentity();
    const parsed = changeSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(
        422,
        "validation",
        "current_password and a new_password of at least 12 characters are required",
      );
    }
    const database = await db();
    const rows = await database
      .select({ passwordHash: schema.users.passwordHash })
      .from(schema.users)
      .where(eq(schema.users.id, identity.user.id))
      .limit(1);
    const current = rows[0];
    if (!current || !(await verifyPassword(current.passwordHash, parsed.data.current_password))) {
      throw new BffFailure(403, "forbidden", "that is not your current password");
    }
    await database
      .update(schema.users)
      .set({ passwordHash: await hashPassword(parsed.data.new_password) })
      .where(eq(schema.users.id, identity.user.id));
    // Every OTHER session ends; this one survives so the user is not logged
    // out of the screen they just used.
    await database
      .delete(schema.sessions)
      .where(
        and(eq(schema.sessions.userId, identity.user.id), ne(schema.sessions.id, identity.sessionId)),
      );
    await writeAudit(identity, "auth.password_change", {});
    return NextResponse.json({ changed: true });
  });
}
