import { NextResponse, type NextRequest } from "next/server";

import { SESSION_COOKIE } from "@/lib/auth/constants";
import { MARKETING_PUBLIC_PATHS } from "@/lib/site/routes";
import { TRACEPARENT_HEADER, resolveTraceparent } from "@/lib/telemetry/traceparent";

// Reachable without a session. The invite and reset flows are how someone
// GETS one, and in both the token in the URL is the credential — a redirect to
// /login would strand the very person the link was sent to.
const PUBLIC_PATHS = [
  "/login",
  "/api/auth/login",
  "/invite",
  "/api/invites",
  "/reset",
  "/api/auth/password-reset",
  // CSP violation reports: the browser posts these with no credentials, and the
  // ones from the login page — before any session exists — are the most useful.
  "/api/csp-report",
  // The container's own readiness probe. A kubelet carries no session cookie, so
  // without this entry the probe gets a 401 and the pod never becomes ready —
  // and the cutover gate that reads it would be waiting on a 401 forever. Safe
  // to expose because the handler answers a boolean and a reason class only; see
  // src/app/api/healthz/route.ts for what it deliberately does not check.
  "/api/healthz",
  // O11's embeddable widget. Both halves are public because the signed,
  // record-scoped, expiring token in the path IS the credential — a redirect to
  // /login would break the iframe for the tenant who embedded it deliberately.
  "/widget",
  "/api/widget",
  // The public marketing surface, plus /robots.txt and /sitemap.xml. Spread
  // from one list so the pages, the sitemap and this gate cannot disagree about
  // what is public; see src/lib/site/routes.ts for why the two metadata routes
  // need explicit entries.
  ...MARKETING_PUBLIC_PATHS,
];

/**
 * Cheap cookie-presence gate only: real session verification happens in route
 * handlers and server layouts (the proxy runtime has no database access).
 * Pages redirect to /login; API routes answer with the JSON envelope.
 * Next 16's `proxy` file convention — the renamed `middleware`.
 *
 * It also guarantees every request carries a usable `traceparent`, which is
 * what lets a click be joined to the SQL it causes. Done HERE because it is
 * the one place every request passes through — including the public ones that
 * return early above, since a trace that starts at /login is still a trace.
 *
 * The incoming header is validated rather than trusted: erserver rejects
 * anything that is not W3C version `00` and stores `None` instead, silently,
 * so forwarding a newer client's `01-` context would break the chain with
 * nothing logged. See resolveTraceparent.
 */
export function proxy(request: NextRequest) {
  const { pathname } = request.nextUrl;

  // Established once, then attached to whichever response this returns.
  const { traceparent } = resolveTraceparent(request.headers.get(TRACEPARENT_HEADER));
  const traced = () => {
    const headers = new Headers(request.headers);
    headers.set(TRACEPARENT_HEADER, traceparent);
    return NextResponse.next({ request: { headers } });
  };

  if (PUBLIC_PATHS.some((path) => pathname === path || pathname.startsWith(`${path}/`))) {
    return traced();
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
    // Every path that reaches here is worth returning to after login. `/` used
    // to be special-cased to an empty query because it was the signed-in
    // dispatcher and `?next=/` would have bounced through it twice; `/` is now
    // the public landing page and never reaches this branch at all.
    login.search = `?next=${encodeURIComponent(pathname)}`;
    return NextResponse.redirect(login);
  }
  return traced();
}

export const config = {
  matcher: ["/((?!_next/static|_next/image|favicon.ico|.*\\.(?:svg|png|ico|webp)$).*)"],
};
