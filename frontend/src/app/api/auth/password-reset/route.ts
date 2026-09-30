import { NextResponse } from "next/server";
import { z } from "zod";

import { hashPassword } from "@/lib/auth/password";
import { confirmPasswordReset, createPasswordReset, resetLink } from "@/lib/bff/members";
import { relayEmail } from "@/lib/bff/relay";
import { BffFailure, respond } from "@/lib/bff/proxy";

const requestSchema = z.object({ email: z.email().max(200) });
const confirmSchema = z.object({ token: z.string().min(1), new_password: z.string().min(12).max(200) });

/**
 * Request a reset link, or confirm one with `token`.
 *
 * The request half is deliberately enumeration-safe: an unknown address gets
 * the same 202 and the same copy as a known one, so the endpoint cannot be
 * used to discover who has an account.
 */
export async function POST(request: Request): Promise<NextResponse> {
  return respond(async () => {
    const body: unknown = (await request.json().catch(() => null)) ?? {};

    const confirm = confirmSchema.safeParse(body);
    if (confirm.success) {
      await confirmPasswordReset(confirm.data.token, confirm.data.new_password, hashPassword);
      return NextResponse.json({ reset: true });
    }

    const asked = requestSchema.safeParse(body);
    if (!asked.success) {
      throw new BffFailure(
        422,
        "validation",
        "either {email} to request a link, or {token, new_password} to confirm",
      );
    }
    const created = await createPasswordReset(asked.data.email);
    if (created) {
      await relayEmail({
        to: asked.data.email,
        template: "password_reset",
        params: { link: resetLink(created.token) },
      });
    }
    return NextResponse.json(
      { requested: true, message: "if that address has an account, a reset link is on its way" },
      { status: 202 },
    );
  });
}
