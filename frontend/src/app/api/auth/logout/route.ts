import { cookies } from "next/headers";
import { NextResponse } from "next/server";

import { destroySession, SESSION_COOKIE } from "@/lib/auth/session";
import { respond } from "@/lib/bff/proxy";

export async function POST(): Promise<NextResponse> {
  return respond(async () => {
    const jar = await cookies();
    const token = jar.get(SESSION_COOKIE)?.value;
    if (token) await destroySession(token);
    const response = NextResponse.json({ ok: true });
    response.cookies.delete(SESSION_COOKIE);
    return response;
  });
}
