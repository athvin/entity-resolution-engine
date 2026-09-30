import "server-only";
import { createHash, randomBytes } from "node:crypto";
import { and, eq, isNull, sql } from "drizzle-orm";
import { ulid } from "ulidx";

import { db, schema } from "@/lib/db";
import { env } from "@/lib/env";
import { BffFailure } from "./proxy";

import type { Membership } from "@/lib/auth/session";

/**
 * Org membership management (design §2.2): list, invite, change role, remove.
 *
 * Two rules are load-bearing and live only here:
 *
 * * **An org never loses its last admin.** Demoting or removing the final
 *   admin would leave a workspace nobody can administer — including nobody who
 *   can invite a new admin — so both paths refuse.
 * * **Invite tokens are hashed at rest.** The raw token exists inside the
 *   emailed link and nowhere else, exactly as session tokens do.
 */

const INVITE_DAYS = 7;
const RESET_HOURS = 1;

export function hashToken(token: string): string {
  return createHash("sha256").update(token).digest("hex");
}

function newToken(): string {
  return randomBytes(32).toString("base64url");
}

export interface MemberRow {
  user_id: string;
  email: string;
  display_name: string;
  role: Membership["role"];
  joined_at: string;
}

export interface PendingInvite {
  invite_id: string;
  email: string;
  role: Membership["role"];
  invited_by: string;
  expires_at: string;
}

export async function listMembers(
  org: string,
): Promise<{ members: MemberRow[]; invites: PendingInvite[] }> {
  const database = await db();
  const members = await database
    .select({
      user_id: schema.users.id,
      email: schema.users.email,
      display_name: schema.users.displayName,
      role: schema.orgMembers.role,
      joined_at: schema.orgMembers.createdAt,
    })
    .from(schema.orgMembers)
    .innerJoin(schema.users, eq(schema.orgMembers.userId, schema.users.id))
    .where(eq(schema.orgMembers.org, org))
    .orderBy(schema.users.email);
  const pending = await database
    .select({
      invite_id: schema.invites.id,
      email: schema.invites.email,
      role: schema.invites.role,
      invited_by: schema.invites.invitedBy,
      expires_at: schema.invites.expiresAt,
    })
    .from(schema.invites)
    .where(and(eq(schema.invites.org, org), isNull(schema.invites.acceptedAt)))
    .orderBy(schema.invites.email);
  return {
    members: members.map((row) => ({ ...row, joined_at: row.joined_at.toISOString() })),
    invites: pending.map((row) => ({ ...row, expires_at: row.expires_at.toISOString() })),
  };
}

async function adminCount(org: string): Promise<number> {
  const database = await db();
  const rows = await database
    .select({ count: sql<number>`count(*)::int` })
    .from(schema.orgMembers)
    .where(and(eq(schema.orgMembers.org, org), eq(schema.orgMembers.role, "admin")));
  return rows[0]?.count ?? 0;
}

/** Refuse the change that would leave `org` with no administrator. */
async function assertNotLastAdmin(org: string, userId: string): Promise<void> {
  const database = await db();
  const rows = await database
    .select({ role: schema.orgMembers.role })
    .from(schema.orgMembers)
    .where(and(eq(schema.orgMembers.org, org), eq(schema.orgMembers.userId, userId)))
    .limit(1);
  const current = rows[0];
  if (!current) throw new BffFailure(404, "not_found", "not a member of this workspace");
  if (current.role === "admin" && (await adminCount(org)) <= 1) {
    throw new BffFailure(
      409,
      "conflict",
      "this is the workspace's last admin — promote someone else first",
    );
  }
}

/** The link an invitee follows; the raw token never touches the database. */
export function inviteLink(token: string): string {
  const base = env().ERWEB_PUBLIC_BASE_URL ?? "";
  return `${base}/invite/${token}`;
}

export async function createInvite(
  org: string,
  email: string,
  role: Membership["role"],
  invitedBy: string,
): Promise<{ invite_id: string; token: string; expires_at: string }> {
  const database = await db();
  const normalized = email.trim().toLowerCase();
  const existing = await database
    .select({ id: schema.users.id })
    .from(schema.users)
    .innerJoin(schema.orgMembers, eq(schema.orgMembers.userId, schema.users.id))
    .where(and(eq(schema.users.email, normalized), eq(schema.orgMembers.org, org)))
    .limit(1);
  if (existing.length > 0) {
    throw new BffFailure(409, "conflict", `${normalized} is already a member of this workspace`);
  }
  // Re-inviting supersedes the outstanding invite rather than stacking a second.
  await database
    .delete(schema.invites)
    .where(
      and(
        eq(schema.invites.org, org),
        eq(schema.invites.email, normalized),
        isNull(schema.invites.acceptedAt),
      ),
    );
  const token = newToken();
  const expiresAt = new Date(Date.now() + INVITE_DAYS * 24 * 60 * 60 * 1000);
  const id = ulid().toLowerCase();
  await database.insert(schema.invites).values({
    id,
    org,
    email: normalized,
    role,
    tokenHash: hashToken(token),
    invitedBy,
    expiresAt,
  });
  return { invite_id: id, token, expires_at: expiresAt.toISOString() };
}

export async function changeRole(
  org: string,
  userId: string,
  role: Membership["role"],
): Promise<void> {
  if (role !== "admin") await assertNotLastAdmin(org, userId);
  const database = await db();
  await database
    .update(schema.orgMembers)
    .set({ role })
    .where(and(eq(schema.orgMembers.org, org), eq(schema.orgMembers.userId, userId)));
}

export async function removeMember(org: string, userId: string): Promise<void> {
  await assertNotLastAdmin(org, userId);
  const database = await db();
  // The membership goes; the account stays, so a re-invite is one click and
  // the person's audit history keeps its subject.
  await database
    .delete(schema.orgMembers)
    .where(and(eq(schema.orgMembers.org, org), eq(schema.orgMembers.userId, userId)));
}

export interface InvitePreview {
  org: string;
  email: string;
  role: Membership["role"];
  needs_account: boolean;
}

export async function readInvite(token: string): Promise<InvitePreview> {
  const database = await db();
  const rows = await database
    .select()
    .from(schema.invites)
    .where(and(eq(schema.invites.tokenHash, hashToken(token)), isNull(schema.invites.acceptedAt)))
    .limit(1);
  const invite = rows[0];
  // One message for absent, used and expired: a probe learns nothing either way.
  if (!invite || invite.expiresAt.getTime() < Date.now()) {
    throw new BffFailure(404, "not_found", "this invitation is no longer valid");
  }
  const account = await database
    .select({ id: schema.users.id })
    .from(schema.users)
    .where(eq(schema.users.email, invite.email))
    .limit(1);
  return {
    org: invite.org,
    email: invite.email,
    role: invite.role,
    needs_account: account.length === 0,
  };
}

/**
 * Accept an invitation: create the account if needed, add the membership.
 *
 * **The token is not proof of identity for an account that already exists.**
 * Possessing the link proves only that — and the link is visible to the admin
 * who created it. So the existing-account path requires the acceptor to be
 * signed in AS that account (`actingUserId`); otherwise nothing is written and
 * the caller is told to sign in first. Minting a session from link possession
 * alone would let any admin take over an existing user — including a super
 * admin, or a member of workspaces this admin cannot see.
 *
 * The no-account path is safe to auto-provision: the invite was sent to that
 * address, and the acceptor chooses the password then and there.
 */
export async function acceptInvite(
  token: string,
  password: string | null,
  displayName: string | null,
  passwordHasher: (password: string) => Promise<string>,
  actingUserId: string | null = null,
): Promise<{ user_id: string; org: string; created_account: boolean }> {
  const database = await db();
  const rows = await database
    .select()
    .from(schema.invites)
    .where(and(eq(schema.invites.tokenHash, hashToken(token)), isNull(schema.invites.acceptedAt)))
    .limit(1);
  const invite = rows[0];
  if (!invite || invite.expiresAt.getTime() < Date.now()) {
    throw new BffFailure(404, "not_found", "this invitation is no longer valid");
  }
  const existing = await database
    .select({ id: schema.users.id })
    .from(schema.users)
    .where(eq(schema.users.email, invite.email))
    .limit(1);

  const existingId = existing[0]?.id;
  let userId: string;
  let createdAccount = false;
  if (existingId === undefined) {
    if (password === null || password.length < 12) {
      throw new BffFailure(422, "validation", "a password of at least 12 characters is required");
    }
    userId = ulid().toLowerCase();
    createdAccount = true;
    await database.insert(schema.users).values({
      id: userId,
      email: invite.email,
      passwordHash: await passwordHasher(password),
      displayName: displayName?.trim() || invite.email,
    });
  } else {
    // The account exists: the link alone must not speak for it.
    if (actingUserId !== existingId) {
      throw new BffFailure(
        403,
        "forbidden",
        `${invite.email} already has an account — sign in as them, then open this link again`,
      );
    }
    userId = existingId;
  }
  await database
    .insert(schema.orgMembers)
    .values({ userId, org: invite.org, role: invite.role, addedBy: invite.invitedBy })
    .onConflictDoNothing();
  await database
    .update(schema.invites)
    .set({ acceptedAt: new Date() })
    .where(eq(schema.invites.id, invite.id));
  return { user_id: userId, org: invite.org, created_account: createdAccount };
}

export async function createPasswordReset(
  email: string,
): Promise<{ token: string; userId: string } | null> {
  const database = await db();
  const rows = await database
    .select({ id: schema.users.id })
    .from(schema.users)
    .where(and(eq(schema.users.email, email.trim().toLowerCase()), isNull(schema.users.disabledAt)))
    .limit(1);
  const user = rows[0];
  if (!user) return null; // the caller still answers 200 — no account enumeration
  const token = newToken();
  await database.insert(schema.passwordResets).values({
    id: ulid().toLowerCase(),
    userId: user.id,
    tokenHash: hashToken(token),
    expiresAt: new Date(Date.now() + RESET_HOURS * 60 * 60 * 1000),
  });
  return { token, userId: user.id };
}

export function resetLink(token: string): string {
  const base = env().ERWEB_PUBLIC_BASE_URL ?? "";
  return `${base}/reset/${token}`;
}

/** Consume a reset token and set the new password; revokes every session. */
export async function confirmPasswordReset(
  token: string,
  password: string,
  passwordHasher: (password: string) => Promise<string>,
): Promise<void> {
  if (password.length < 12) {
    throw new BffFailure(422, "validation", "a password of at least 12 characters is required");
  }
  const database = await db();
  const rows = await database
    .select()
    .from(schema.passwordResets)
    .where(
      and(eq(schema.passwordResets.tokenHash, hashToken(token)), isNull(schema.passwordResets.usedAt)),
    )
    .limit(1);
  const reset = rows[0];
  if (!reset || reset.expiresAt.getTime() < Date.now()) {
    throw new BffFailure(404, "not_found", "this reset link is no longer valid");
  }
  await database
    .update(schema.users)
    .set({ passwordHash: await passwordHasher(password) })
    .where(eq(schema.users.id, reset.userId));
  await database
    .update(schema.passwordResets)
    .set({ usedAt: new Date() })
    .where(eq(schema.passwordResets.id, reset.id));
  // A password change ends every session: whoever knew the old one is out.
  await database.delete(schema.sessions).where(eq(schema.sessions.userId, reset.userId));
}
