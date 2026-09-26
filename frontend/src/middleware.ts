import { NextResponse, type NextRequest } from "next/server";

export const SESSION_COOKIE = "er_session";

const PUBLIC_PATHS = ["/login", "/api/auth/login"];

/**
 * Cheap cookie-presence gate only: real session verification happens in route
 * handlers and server layouts (the middleware runtime has no database access).
 */
export function middleware(request: NextRequest) {
  const { pathname } = request.nextUrl;
  if (PUBLIC_PATHS.some((path) => pathname === path || pathname.startsWith(`${path}/`))) {
    return NextResponse.next();
  }
  if (!request.cookies.has(SESSION_COOKIE)) {
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
