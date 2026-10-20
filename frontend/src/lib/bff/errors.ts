/**
 * The single error envelope every BFF route returns. Codes are the client's
 * switch surface; the mapping from erserver statuses lands with the proxy (M1):
 * 401→unauthorized, 403→forbidden, 404→not_found,
 * 409→org_not_active|writer_locked|conflict, 413→payload_too_large,
 * 422→validation (with JSON pointer), 503→lake_unavailable, and the
 * fresh-tenant import failure surfaces as training_required.
 *
 * `conflict` is the BFF's own 409: a request the caller could legitimately
 * make, refused by the state it would produce — the last admin of a workspace
 * being demoted, an already-member being invited again.
 */
export const BFF_ERROR_CODES = [
  "unauthorized",
  "forbidden",
  "not_found",
  "org_not_active",
  "writer_locked",
  "conflict",
  "payload_too_large",
  "validation",
  "lake_unavailable",
  "training_required",
  "internal",
] as const;

export type BffErrorCode = (typeof BFF_ERROR_CODES)[number];

export interface BffError {
  code: BffErrorCode;
  message: string;
  detail?: unknown;
  pointer?: string;
}

export function isBffError(value: unknown): value is BffError {
  // Object.hasOwn rather than `in`, which walks the prototype chain. This is
  // belt-and-braces, not a fix for a live bug: the only thing this guard is ever
  // handed is `JSON.parse` output, which has no inherited properties. But a
  // trust-boundary guard should mean "this object carries a code", and `in` does
  // not say that.
  if (typeof value !== "object" || value === null || !Object.hasOwn(value, "code")) return false;
  const code = (value as { code: unknown }).code;
  return typeof code === "string" && (BFF_ERROR_CODES as readonly string[]).includes(code);
}
