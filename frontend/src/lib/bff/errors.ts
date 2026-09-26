/**
 * The single error envelope every BFF route returns. Codes are the client's
 * switch surface; the mapping from erserver statuses lands with the proxy (M1):
 * 401→unauthorized, 403→forbidden, 404→not_found, 409→org_not_active|writer_locked,
 * 413→payload_too_large, 422→validation (with JSON pointer), 503→lake_unavailable,
 * and the fresh-tenant import failure surfaces as training_required.
 */
export const BFF_ERROR_CODES = [
  "unauthorized",
  "forbidden",
  "not_found",
  "org_not_active",
  "writer_locked",
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
  if (typeof value !== "object" || value === null || !("code" in value)) return false;
  const code = value.code;
  return typeof code === "string" && (BFF_ERROR_CODES as readonly string[]).includes(code);
}
