import "server-only";

import { env } from "@/lib/env";

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
 * P19 adds `traceparent` and the session id, and they must arrive through
 * `AsyncLocalStorage` (`node:async_hooks`) rather than being read here from
 * `next/headers`. `headers()` is a Request-time API in Next 16: calling it opts
 * its caller into dynamic rendering, and `forward` is reachable from a server
 * component, so reading it inside this function would change rendering
 * behaviour for pages that merely fetch. Verified against
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
