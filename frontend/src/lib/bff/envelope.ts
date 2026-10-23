/**
 * Unwrapping er-server's list envelopes.
 *
 * er-server has no single convention: `/jobs` wraps in `{jobs}`,
 * `/webhooks/deliveries` in `{deliveries}`, `/webhooks/events` in `{events}`,
 * the read-API pages in `{items}`, and `/webhooks` returns a bare array. The
 * browser contract is a bare array everywhere, so each BFF route unwraps — and
 * getting the key wrong is invisible, because the mock encodes the frontend's
 * belief rather than the server's behaviour.
 *
 * That exact mistake shipped: the jobs route was `forward<unknown[]>` over
 * `{jobs: […]}`, a type ASSERTION about an HTTP body TypeScript cannot check,
 * so the runs page's table rendered nothing against the real backend while
 * every mock-backed tier was green. This module exists so the unwrap is a
 * function with a test rather than a cast with a comment.
 */

/**
 * The array at `body[key]`, or `body` itself when it is already one.
 *
 * Tolerating both shapes is deliberate rather than lazy: it is what makes a
 * route survive the envelope being *added* to a response that did not have
 * one, which is a change the control plane has made before. The empty array is
 * the answer for a missing or non-array value — a list endpoint that answered
 * `null` would otherwise crash a `.map()` three components deep, far from the
 * route that misread it.
 */
export function listFrom<T>(body: unknown, key: string): T[] {
  if (Array.isArray(body)) {
    return body as T[];
  }
  if (body !== null && typeof body === "object") {
    const inner = (body as Record<string, unknown>)[key];
    if (Array.isArray(inner)) {
      return inner as T[];
    }
  }
  return [];
}
