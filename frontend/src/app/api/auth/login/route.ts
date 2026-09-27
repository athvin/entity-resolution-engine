import { eq } from "drizzle-orm";
import { NextResponse } from "next/server";
import { ulid } from "ulidx";
import { z } from "zod";

import { verifyPassword } from "@/lib/auth/password";
import { createSession, SESSION_COOKIE, sessionCookieOptions } from "@/lib/auth/session";
import { BffFailure, respond } from "@/lib/bff/proxy";
import { db, schema } from "@/lib/db";

const credentialsSchema = z.object({ email: z.string().min(3), password: z.string().min(1) });

export async function POST(request: Request): Promise<NextResponse> {
  return respond(async () => {
    const parsed = credentialsSchema.safeParse(await request.json().catch(() => null));
    if (!parsed.success) {
      throw new BffFailure(422, "validation", "email and password are required");
    }
    const database = await db();
    const rows = await database
      .select()
      .from(schema.users)
      .where(eq(schema.users.email, parsed.data.email.toLowerCase()))
      .limit(1);
    const user = rows[0];
    if (
      user === undefined ||
      user.disabledAt !== null ||
      !(await verifyPassword(user.passwordHash, parsed.data.password))
    ) {
      // One message for every failure mode: login errors do not explain themselves.
      throw new BffFailure(401, "unauthorized", "invalid email or password");
    }
    const token = await createSession(user.id);
    await database.insert(schema.audit).values({
      id: ulid().toLowerCase(),
      userId: user.id,
      actingRole: user.isSuperAdmin ? "super_admin" : "user",
      action: "auth.login",
      detail: {},
    });

    // Default landing: the first membership; super admins fall back to the
    // first registered workspace (the console proper arrives in M2).
    const memberships = await database
      .select({ org: schema.orgMembers.org })
      .from(schema.orgMembers)
      .where(eq(schema.orgMembers.userId, user.id))
      .orderBy(schema.orgMembers.org)
      .limit(1);
    let target = memberships[0] ? `/${memberships[0].org}/dashboard` : null;
    if (!target && user.isSuperAdmin) {
      const registered = await database.select().from(schema.orgsRegistry).limit(1);
      target = registered[0] ? `/${registered[0].org}/dashboard` : null;
    }

    const response = NextResponse.json({
      ok: true,
      redirect: target ?? "/login?error=no-workspace",
    });
    response.cookies.set(SESSION_COOKIE, token, sessionCookieOptions());
    return response;
  });
}
