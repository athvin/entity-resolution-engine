import { NextResponse, type NextRequest } from "next/server";

import { SESSION_COOKIE } from "@/lib/auth/constants";

// Reachable without a session. The invite and reset flows are how someone
// GETS one, and in both the token in the URL is the credential — a redirect to
// /login would strand the very person the link was sent to.
const PUBLIC_PATHS = [
  "/login",
  "/api/auth/login",
  "/invite",
  "/api/invites",
  "/api/auth/password-reset",
];

/**
 * Cheap cookie-presence gate only: real session verification happens in route
 * handlers and server layouts (the proxy runtime has no database access).
 * Pages redirect to /login; API routes answer with the JSON envelope.
 * Next 16's `proxy` file convention — the renamed `middleware`.
 */
export function proxy(request: NextRequest) {
  const { pathname } = request.nextUrl;
  if (PUBLIC_PATHS.some((path) => pathname === path || pathname.startsWith(`${path}/`))) {
    return NextResponse.next();
  }
  if (!request.cookies.has(SESSION_COOKIE)) {
    if (pathname.startsWith("/api/")) {
      return NextResponse.json(
        { code: "unauthorized", message: "sign in to continue" },
        { status: 401 },
      );
    }
    const login = request.nextUrl.clone();
    login.pathname = "/login";
    login.search = pathname === "/" ? "" : `?next=${encodeURIComponent(pathname)}`;
    return NextResponse.redirect(login);
  }
  return NextResponse.next();
}

export const config = {
  matcher: ["/((?!_next/static|_next/image|favicon.ico|.*\\.(?:svg|png|ico|webp)$).*)"],
};
