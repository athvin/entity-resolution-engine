import { NextResponse } from "next/server";
import { z } from "zod";

import { hashPassword } from "@/lib/auth/password";
import {
  createSession,
  identityFromCookies,
  sessionCookieOptions,
  SESSION_COOKIE,
} from "@/lib/auth/session";
import { acceptInvite, readInvite } from "@/lib/bff/members";
import { BffFailure, respond } from "@/lib/bff/proxy";

/** Both halves are deliberately unauthenticated: the token IS the credential. */

export async function GET(
  _request: Request,
  { params }: { params: Promise<{ token: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { token } = await params;
    return NextResponse.json(await readInvite(token));
  });
}

const acceptSchema = z.object({
  password: z.string().min(12).max(200).optional(),
  display_name: z.string().max(120).optional(),
});

export async function POST(
  request: Request,
  { params }: { params: Promise<{ token: string }> },
): Promise<NextResponse> {
  return respond(async () => {
    const { token } = await params;
    const parsed = acceptSchema.safeParse((await request.json().catch(() => null)) ?? {});
    if (!parsed.success) {
      throw new BffFailure(422, "validation", "password must be at least 12 characters");
    }
    // Whoever is already signed in, if anyone — the existing-account path needs
    // it to prove the acceptor IS the invitee rather than merely holding a link.
    const identity = await identityFromCookies();
    const result = await acceptInvite(
      token,
      parsed.data.password ?? null,
      parsed.data.display_name ?? null,
      hashPassword,
      identity?.user.id ?? null,
    );
    const response = NextResponse.json(result, { status: 201 });
    if (result.created_account) {
      // Only the account we just created, whose password the acceptor chose on
      // this screen, is signed in here. An existing account already had to be
      // signed in to get this far, so it keeps the session it arrived with.
      const sessionToken = await createSession(result.user_id);
      response.cookies.set(SESSION_COOKIE, sessionToken, sessionCookieOptions());
    }
    return response;
  });
}
