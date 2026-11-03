import { NextResponse } from "next/server";

import { probeReadiness } from "@/lib/bff/readiness";
import { upstreamOrigin } from "@/lib/bff/upstream";

/**
 * The BFF's readiness probe: can *this container* serve a request?
 *
 * The decision and its reasoning live in `src/lib/bff/readiness.ts` (so the
 * branches that matter — a timeout, a refused connection — are reachable from
 * the unit tier); this is the wiring. Two things are checked, and both are
 * properties of this container rather than of the deployment:
 *
 * 1. **The environment parses.** `upstreamOrigin()` goes through `env()`, which
 *    validates the whole schema on its first call — it is lazy so `next build`
 *    succeeds without a runtime env, see `src/lib/env.ts` — so a missing
 *    ERWEB_SESSION_SECRET or a malformed ERWEB_CREDENTIAL_KEY surfaces here
 *    instead of on the first real request. A container that cannot read its own
 *    secrets must never enter the Service.
 * 2. **The upstream in this pod answers.** er-server runs in the same pod and
 *    is reached over localhost, so this is not a network check — it is the one
 *    question `/` cannot answer: did the sibling container finish booting?
 *
 * Unauthenticated, and listed in PUBLIC_PATHS in `src/proxy.ts`: a kubelet
 * probe carries no session cookie, and without that entry the proxy answers 401
 * and the pod never becomes ready. Safe to expose because the body is a boolean
 * and a reason class — anyone who can reach the pod can reach this.
 *
 * Not cached: route handlers are uncached by default in this Next version, and
 * this one does a network request besides, so nothing would prerender it. The
 * `no-store` header is for everything between the kubelet and here.
 */
export async function GET(): Promise<NextResponse> {
  let baseUrl: string | undefined;
  try {
    baseUrl = upstreamOrigin();
  } catch {
    // Left undefined so the helper owns the verdict. The parse error itself is
    // a deploy-time fact and reaches the pod log from the first real request;
    // repeating it in a probe body would leak configuration to anyone who can
    // reach the pod.
    baseUrl = undefined;
  }

  const readiness = await probeReadiness({ baseUrl });
  return NextResponse.json(readiness, {
    status: readiness.ready ? 200 : 503,
    headers: { "cache-control": "no-store" },
  });
}
