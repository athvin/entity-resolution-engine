import { NextResponse } from "next/server";

/**
 * The sink for Content-Security-Policy-Report-Only violations (next.config.ts).
 *
 * The policy ships report-only so it cannot break the app; this is what makes it
 * worth anything — without somewhere to send reports, a report-only CSP is just
 * a header. Reports are logged, not stored: the question they answer is "does
 * the realistic policy hold in practice", which is a question about the log, and
 * a tenant-visible table of them would be a new surface to secure for no gain.
 *
 * Deliberately unauthenticated, and listed in PUBLIC_PATHS in src/proxy.ts. A
 * violation on the login page happens before there is a session, and those are
 * among the reports most worth having. The trade is that anyone can post here,
 * so this handler must stay cheap and must not trust the body: it is size-capped
 * and the payload is never interpolated into anything, only logged.
 */

/** Browsers post a few hundred bytes. 16 KiB is slack, not a budget. */
const MAX_BYTES = 16 * 1024;

export async function POST(request: Request): Promise<NextResponse> {
  const length = Number(request.headers.get("content-length") ?? "0");
  if (length > MAX_BYTES) {
    return new NextResponse(null, { status: 413 });
  }

  let body: string;
  try {
    body = await request.text();
  } catch {
    return new NextResponse(null, { status: 204 });
  }

  if (body.length > MAX_BYTES) {
    return new NextResponse(null, { status: 413 });
  }

  // Both shapes exist in the wild: the legacy `report-uri` body is
  // `{"csp-report": {...}}`, the Reporting-API `report-to` body is an array of
  // `{type, body}`. Log whichever arrived rather than normalizing — the point is
  // to read these, and a parse that drops a field is worse than a verbose line.
  console.warn("csp-report:", body.slice(0, MAX_BYTES));

  // 204: the reporting spec ignores the response body, and returning one would
  // only invite someone to probe this endpoint for behavior.
  return new NextResponse(null, { status: 204 });
}
