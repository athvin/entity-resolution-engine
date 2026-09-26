import "server-only";
import { createHash, randomBytes } from "node:crypto";
import { and, eq, gt, isNull } from "drizzle-orm";
import { cookies } from "next/headers";
import { ulid } from "ulidx";

import { db, schema } from "@/lib/db";
import { SESSION_COOKIE } from "./constants";

import type { Membership, SessionUser } from "./types";

const SESSION_DAYS = 30;
const SLIDE_AFTER_MS = 60 * 60 * 1000; // bump last-seen/expiry at most hourly

export type { Membership, SessionUser };

export interface Identity {
  sessionId: string;
  user: SessionUser;
  memberships: Membership[];
  impersonating: { org: string; role: Membership["role"]; expiresAt: Date } | null;
}

function tokenHash(token: string): string {
  return createHash("sha256").update(token).digest("hex");
}

function expiry(): Date {
  return new Date(Date.now() + SESSION_DAYS * 24 * 60 * 60 * 1000);
}

/** Opaque 256-bit token; only its sha256 is stored. Returns the cookie value. */
export async function createSession(userId: string): Promise<string> {
  const token = randomBytes(32).toString("base64url");
  const database = await db();
  await database.insert(schema.sessions).values({
    id: ulid().toLowerCase(),
    userId,
    tokenHash: tokenHash(token),
    expiresAt: expiry(),
  });
  return token;
}

export async function destroySession(token: string): Promise<void> {
  const database = await db();
  await database.delete(schema.sessions).where(eq(schema.sessions.tokenHash, tokenHash(token)));
}

/** Resolve the cookie to a full identity, or null. Slides expiry lazily. */
export async function identityFromCookies(): Promise<Identity | null> {
  const jar = await cookies();
  const token = jar.get(SESSION_COOKIE)?.value;
  if (!token) return null;
  const database = await db();
  const rows = await database
    .select({ session: schema.sessions, user: schema.users })
    .from(schema.sessions)
    .innerJoin(schema.users, eq(schema.sessions.userId, schema.users.id))
    .where(
      and(
        eq(schema.sessions.tokenHash, tokenHash(token)),
        gt(schema.sessions.expiresAt, new Date()),
        isNull(schema.users.disabledAt),
      ),
    )
    .limit(1);
  const row = rows[0];
  if (!row) return null;

  if (Date.now() - row.session.lastSeenAt.getTime() > SLIDE_AFTER_MS) {
    await database
      .update(schema.sessions)
      .set({ lastSeenAt: new Date(), expiresAt: expiry() })
      .where(eq(schema.sessions.id, row.session.id));
  }

  const memberships = await database
    .select({ org: schema.orgMembers.org, role: schema.orgMembers.role })
    .from(schema.orgMembers)
    .where(eq(schema.orgMembers.userId, row.user.id));

  const impersonation =
    row.session.impersonatingOrg &&
    row.session.impersonatedRole &&
    row.session.impersonationExpiresAt &&
    row.session.impersonationExpiresAt.getTime() > Date.now()
      ? {
          org: row.session.impersonatingOrg,
          role: row.session.impersonatedRole,
          expiresAt: row.session.impersonationExpiresAt,
        }
      : null;

  return {
    sessionId: row.session.id,
    user: {
      id: row.user.id,
      email: row.user.email,
      displayName: row.user.displayName,
      isSuperAdmin: row.user.isSuperAdmin,
    },
    memberships,
    impersonating: impersonation,
  };
}

export function sessionCookieOptions() {
  return {
    httpOnly: true as const,
    sameSite: "lax" as const,
    // Secure by default. NODE_ENV is useless here (`next start` is always
    // "production", including on a laptop), so plain-HTTP environments — local
    // dev and the Playwright tiers — opt out explicitly.
    secure: process.env.ERWEB_INSECURE_COOKIES !== "1",
    path: "/",
    maxAge: SESSION_DAYS * 24 * 60 * 60,
  };
}

export { SESSION_COOKIE };
