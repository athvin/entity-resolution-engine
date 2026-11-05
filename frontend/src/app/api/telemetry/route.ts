import { NextResponse } from "next/server";

import { requireIdentity, respond } from "@/lib/bff/proxy";
import { isAcceptableTraceparent } from "@/lib/telemetry/traceparent";

/**
 * The click stream's ingest: the browser beacons interactions here, and the
 * server forwards them to the collector.
 *
 * ## Why same-origin and not straight to the collector
 *
 * Direct browser-to-collector is not an option here and would not be a good one
 * anywhere. The collector is a cluster-internal Service with `ingress.enabled:
 * false`, dev is Tailscale-internal, and making it reachable would mean a
 * public unauthenticated telemetry ingest in front of the one component that
 * bills by the byte.
 *
 * Going through the BFF also gets three things for free that a direct path
 * could not have: the session cookie, so an interaction is attributable to a
 * person without the browser being trusted to say who; the org scoping; and a
 * server-side allowlist on what may be recorded, which the browser cannot
 * bypass.
 *
 * ## What the browser is NOT trusted for
 *
 * The session id and the user come from the httpOnly cookie, read here — never
 * from the body. `Identity.sessionId` is deliberately never passed to the
 * browser, so it cannot be forged into a beacon.
 *
 * The action name is bounded and the traceparent is validated against the same
 * rule erserver applies. Anything else in the body is ignored rather than
 * forwarded: a field nobody reads is a field nobody has to sanitise.
 */

/** A beacon is a few hundred bytes per interaction; this is slack, not a budget. */
const MAX_BYTES = 64 * 1024;

/** Above this an action name is not a label, whatever it is. */
const MAX_ACTION_LENGTH = 64;

/** One batch cannot describe more clicks than the browser's own queue holds. */
const MAX_INTERACTIONS = 200;

interface Accepted {
  traceparent: string;
  action: string;
  source: string;
  at: number;
}

/**
 * Keep only what is both recognised and safe.
 *
 * Returns the accepted interactions; anything malformed is dropped silently
 * rather than 400'd. A beacon has no one to report an error to — the page may
 * already be gone — so rejecting the batch would lose the good entries with the
 * bad.
 */
function accept(body: unknown): Accepted[] {
  if (typeof body !== "object" || body === null) return [];
  const raw = (body as { interactions?: unknown }).interactions;
  if (!Array.isArray(raw)) return [];

  return raw.slice(0, MAX_INTERACTIONS).flatMap((entry): Accepted[] => {
    if (typeof entry !== "object" || entry === null) return [];
    const { traceparent, action, source, at } = entry as Record<string, unknown>;
    if (typeof traceparent !== "string" || !isAcceptableTraceparent(traceparent)) return [];
    if (typeof action !== "string" || action.length === 0) return [];
    return [
      {
        traceparent,
        action: action.slice(0, MAX_ACTION_LENGTH),
        source: typeof source === "string" ? source.slice(0, 32) : "unknown",
        at: typeof at === "number" && Number.isFinite(at) ? at : Date.now(),
      },
    ];
  });
}

export async function POST(request: Request): Promise<NextResponse> {
  return respond(async () => {
    // The beacon writes session-attributed records, so it authenticates. It is
    // NOT in UNAUTHENTICATED_DIRS beside csp-report for exactly that reason.
    const identity = await requireIdentity();

    const length = Number(request.headers.get("content-length") ?? "0");
    if (length > MAX_BYTES) return new NextResponse(null, { status: 413 });

    let body: unknown;
    try {
      body = await request.json();
    } catch {
      return new NextResponse(null, { status: 204 });
    }

    const interactions = accept(body);
    const reportedDropped = (body as { dropped?: unknown }).dropped;
    const dropped = typeof reportedDropped === "number" ? reportedDropped : 0;

    if (interactions.length === 0 && dropped === 0) {
      return new NextResponse(null, { status: 204 });
    }

    // Forwarding to the collector is the next step and is deliberately not
    // here yet: it needs an OTLP encoder on the Node side, and landing the
    // ingest first means the browser half can be exercised — and its PII
    // boundary reviewed — before anything is exported. Until then the
    // interactions are logged, which Fluent Bit already ships in-account.
    //
    // `er.session.id` and the user come from the COOKIE, never the body.
    console.info(
      JSON.stringify({
        msg: "ui.interactions",
        "er.user.id": identity.user.id,
        "er.session.id": identity.sessionId,
        dropped,
        interactions: interactions.map((i) => ({
          traceparent: i.traceparent,
          "er.action": i.action,
          "er.action.source": i.source,
          at: i.at,
        })),
      }),
    );

    return new NextResponse(null, { status: 204 });
  });
}
