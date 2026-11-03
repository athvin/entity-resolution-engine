import "server-only";

import { env } from "@/lib/env";
import { currentRequestContext } from "@/lib/telemetry/context";
import { TRACEPARENT_HEADER } from "@/lib/telemetry/traceparent";

/**
 * The one place the BFF builds a request to erserver.
 *
 * ## Why this exists
 *
 * Before this, five call sites each built the URL and the header set by hand —
 * `rawFetch` (the tenant proxy), `operatorFetch` (the console), `relayEmail`,
 * the imports multipart route and the public widget route — plus a dead sixth
 * (`lib/api/erserver.ts`, deleted with this change: a path with no callers is
 * still a path the next person can revive). They had already drifted: four set
 * `cache: "no-store"` and one did not, and anything that needs to be attached
 * to *every* upstream call had to be added in five places and could silently
 * be added in four.
 *
 * That last part is the reason this is a refactor worth making on its own. The
 * telemetry work needs `traceparent` on every upstream request; with five
 * paths, "every" is a thing you hope for rather than a thing you can gate.
 *
 * ## What stays with the caller, and why
 *
 * `authorization` is a parameter rather than something resolved here, because
 * the three credential stories are genuinely different and collapsing them
 * would be the wrong kind of sharing: the tenant proxy selects a stored org key
 * by role, the console uses the operator token, and the widget's signed token
 * *is* the credential. Error mapping stays with the caller for the same reason
 * — `relayEmail` returns `false`, the widget returns a 404 that leaks nothing,
 * and the proxy raises a typed `BffFailure`. This function does not catch:
 * every caller already has the `try`/`catch` that knows what its failure means.
 *
 * ## `?: T | undefined` rather than `?: T`
 *
 * `tsconfig.json` sets `exactOptionalPropertyTypes`, so an explicit `undefined`
 * is not assignable to a plain optional property. Callers forward their own
 * optionals straight through (`method: options.method`), and writing the unions
 * out keeps that readable at five call sites instead of making each one build a
 * conditional object.
 *
 * ## Where trace context attaches, and why not here yet
 *
 * `traceparent` arrives through `AsyncLocalStorage`, established by `respond()`
 * — NOT read here from `next/headers`. `headers()` is a Request-time API in
 * Next 16: calling it opts its caller into dynamic rendering, and `forward` is
 * reachable from a server component, so reading it inside this function would
 * change rendering behaviour for pages that merely fetch. Verified against
 * `node_modules/next/dist/docs/01-app/03-api-reference/04-functions/headers.md`
 * rather than assumed — this Next major differs from training data and
 * `AGENTS.md` says to read the shipped docs.
 *
 * Headers that nothing reads yet are deliberately **not** added here. An
 * unread `x-er-dataset` would be a third dead primitive of the kind this
 * change is removing, so it lands when erserver reads it.
 */
export interface UpstreamCall {
  /** Path on erserver, resolved against `ERSERVER_BASE_URL`. */
  path: string;
  /** The full header value, e.g. `Bearer …`. Caller-selected; see above. */
  authorization: string;
  method?: string | undefined;
  searchParams?: URLSearchParams | undefined;
  /**
   * A JSON body. Sets `content-type: application/json` and stringifies.
   * Mutually exclusive with `body`.
   */
  json?: unknown;
  /**
   * A pre-encoded body, passed through verbatim with **no** `content-type`.
   * That omission is load-bearing for `FormData`: `fetch` generates the
   * multipart boundary itself, and setting the header by hand produces a body
   * the upstream cannot parse. Mutually exclusive with `json`.
   */
  body?: BodyInit | undefined;
  idempotencyKey?: string | undefined;
  /**
   * The person the call acts for, sent as `x-acting-user`.
   *
   * Omitted entirely when the credential is itself the identity — the widget
   * route has no logged-in person. Note erserver's own doc comment: this header
   * is spoofable by any API-key holder by design, so it is a trail and not an
   * authorization input. Person-level attribution comes from `erweb.audit`,
   * written from the httpOnly cookie.
   */
  actingUser?: string | undefined;
}

/** Build and send an erserver request. Returns the raw `Response`, OK or not. */
export async function upstreamFetch(call: UpstreamCall): Promise<Response> {
  if (call.json !== undefined && call.body !== undefined) {
    // A programming error rather than a request error, so it throws plainly
    // instead of becoming a status: a body sent two ways has no right answer.
    throw new TypeError("upstreamFetch: pass either `json` or `body`, not both");
  }

  const url = new URL(call.path, env().ERSERVER_BASE_URL);
  if (call.searchParams) url.search = call.searchParams.toString();

  const headers: Record<string, string> = { authorization: call.authorization };

  // The trace context, if this call is inside a request. `undefined` is a
  // normal answer — a cron or a handler outside `respond()` has none — and in
  // that case nothing is attached rather than a fabricated parent being
  // invented, because that would assert a causal link that does not exist.
  const context = currentRequestContext();
  if (context) headers[TRACEPARENT_HEADER] = context.traceparent;

  if (call.actingUser !== undefined) headers["x-acting-user"] = call.actingUser;
  if (call.json !== undefined) headers["content-type"] = "application/json";
  if (call.idempotencyKey !== undefined) headers["idempotency-key"] = call.idempotencyKey;

  const body: BodyInit | null =
    call.json !== undefined ? JSON.stringify(call.json) : (call.body ?? null);

  return fetch(url, {
    method: call.method ?? "GET",
    headers,
    body,
    // Every one of these is per-request and per-identity. One of the five
    // original paths omitted this; a cached control-plane response is a
    // correctness bug, not a performance win.
    cache: "no-store",
  });
}

/**
 * The upstream origin, for the one consumer that must NOT build a request.
 *
 * This module owns the knowledge of where erserver is — that is the invariant
 * `scripts/check-bff-upstream.mjs` enforces, and it still holds: nothing else
 * names `ERSERVER_BASE_URL`. What this adds is a second, deliberate consumer of
 * that knowledge, and it is worth reading why it is not a sixth `upstreamFetch`
 * caller.
 *
 * The readiness probe (`src/lib/bff/readiness.ts`, served at `/api/healthz`) is
 * the opposite of every call above in three ways, each of which `upstreamFetch`
 * would get wrong:
 *
 * - **It carries no credential.** `/readyz` is anonymous by design, and
 *   `authorization` here is a required parameter precisely so a real call
 *   cannot forget one. Making it optional to fit a probe would weaken that for
 *   every caller, to accommodate the one case that must not have it — a bad
 *   trade, and the reason this is an accessor rather than a new parameter.
 * - **It needs a deadline.** A probe that hangs is a probe that says nothing;
 *   `upstreamFetch` has no timeout because a tenant request should not be cut
 *   off at an arbitrary one.
 * - **It must not carry trace context.** When P19 attaches `traceparent` to
 *   every upstream call, a probe running every few seconds would be ~780k
 *   no-information spans a month — which is why er-server's own `is_probe`
 *   already drops them at the source.
 *
 * So: one module still knows the address, and the probe stays the thing it has
 * to be.
 */
export function upstreamOrigin(): string {
  return env().ERSERVER_BASE_URL;
}
