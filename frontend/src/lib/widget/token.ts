import "server-only";
import { createHmac, timingSafeEqual } from "node:crypto";

import { env } from "@/lib/env";

/**
 * Signed tokens for the embeddable duplicate widget (O11).
 *
 * The widget is the product's ONLY surface served without a session: a tenant
 * drops an iframe into their own CRM and it shows "possible duplicates for
 * record X". So the token in the URL is the entire credential, and every
 * decision here follows from that.
 *
 * **It is scoped to one record in one dataset, not to an org.** A token that said "this org"
 * would, if it leaked from one embed, read the whole corpus. Scoping it to the
 * record it was minted for means a leaked token exposes the page the holder
 * could already see.
 *
 * **It expires.** An embed lives in a page a tenant controls and we do not, so
 * a token in a browser history or a referrer header has to stop working.
 *
 * **It is read-only by construction**: there is no widget route that writes,
 * and the token carries no role to escalate.
 */

export interface WidgetClaims {
  org: string;
  dataset: string;
  recordKey: string;
  /** Unix seconds. */
  exp: number;
}

/** How long a minted token lives. A day is long enough for a page to be loaded
 * and short enough that a leaked URL stops working before anyone finds it. */
export const DEFAULT_TTL_SECONDS = 24 * 60 * 60;

/** The longest a tenant may ask for. Beyond this a token is a standing grant,
 * which is what the expiry exists to avoid. */
export const MAX_TTL_SECONDS = 30 * 24 * 60 * 60;

/**
 * The signing key.
 *
 * Derived from `ERWEB_SESSION_SECRET` with a distinct label rather than used
 * directly: a widget token and a session cookie must not be interchangeable
 * even if one of the two formats is ever confused for the other. The label is
 * the domain separation.
 */
function signingKey(): Buffer {
  return createHmac("sha256", env().ERWEB_SESSION_SECRET).update("erweb.widget.v1").digest();
}

function body(claims: WidgetClaims): string {
  // Base64url of a compact JSON object. Order is fixed by construction rather
  // than by `JSON.stringify` over a built object, so the signed bytes cannot
  // change because a key was added somewhere upstream.
  return Buffer.from(
    JSON.stringify({ o: claims.org, d: claims.dataset, r: claims.recordKey, e: claims.exp }),
    "utf8",
  ).toString("base64url");
}

function sign(payload: string): string {
  return createHmac("sha256", signingKey()).update(payload).digest("base64url");
}

/** Mint a token for one record. */
export function mintWidgetToken(
  org: string,
  dataset: string,
  recordKey: string,
  ttlSeconds: number = DEFAULT_TTL_SECONDS,
): string {
  const exp = Math.floor(Date.now() / 1000) + Math.min(ttlSeconds, MAX_TTL_SECONDS);
  const payload = body({ org, dataset, recordKey, exp });
  return `${payload}.${sign(payload)}`;
}

export type TokenFailure = "malformed" | "bad_signature" | "expired";

/**
 * Verify a token, or say why it is not usable.
 *
 * The signature is compared with `timingSafeEqual` — a widget token is checked
 * on an unauthenticated endpoint an attacker can call freely, which is exactly
 * the setting where a byte-at-a-time comparison leaks the signature.
 *
 * **The signature is checked BEFORE the expiry.** Checking expiry first would
 * answer "expired" for a forged token, which tells a forger their signature
 * was accepted.
 */
export function verifyWidgetToken(
  token: string,
): { ok: true; claims: WidgetClaims } | { ok: false; failure: TokenFailure } {
  const dot = token.lastIndexOf(".");
  if (dot <= 0 || dot === token.length - 1) return { ok: false, failure: "malformed" };
  const payload = token.slice(0, dot);
  const provided = token.slice(dot + 1);

  const expected = sign(payload);
  const a = Buffer.from(provided, "utf8");
  const b = Buffer.from(expected, "utf8");
  // `timingSafeEqual` throws on a length mismatch, which is itself a timing
  // signal — but length is public here (the digest is fixed-width), so a
  // mismatched length is simply malformed.
  if (a.length !== b.length || !timingSafeEqual(a, b)) {
    return { ok: false, failure: "bad_signature" };
  }

  let parsed: unknown;
  try {
    parsed = JSON.parse(Buffer.from(payload, "base64url").toString("utf8"));
  } catch {
    return { ok: false, failure: "malformed" };
  }
  if (typeof parsed !== "object" || parsed === null) return { ok: false, failure: "malformed" };
  const record = parsed as Record<string, unknown>;
  const org = record["o"];
  const dataset = record["d"];
  const recordKey = record["r"];
  const exp = record["e"];
  if (
    typeof org !== "string" ||
    typeof dataset !== "string" ||
    typeof recordKey !== "string" ||
    typeof exp !== "number"
  ) {
    return { ok: false, failure: "malformed" };
  }
  if (org === "" || dataset === "" || recordKey === "") return { ok: false, failure: "malformed" };

  if (exp * 1000 <= Date.now()) return { ok: false, failure: "expired" };
  return { ok: true, claims: { org, dataset, recordKey, exp } };
}
