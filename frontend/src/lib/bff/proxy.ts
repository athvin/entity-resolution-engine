import "server-only";
import { NextResponse } from "next/server";

import { identityFromCookies, type Identity, type Membership } from "@/lib/auth/session";
import { env } from "@/lib/env";
import type { BffError, BffErrorCode } from "./errors";

/**
 * The heart of the BFF: session → effective identity → credential → typed
 * fetch to erserver → the shared error envelope. Route handlers stay thin.
 */

const ROLE_RANK: Record<Membership["role"], number> = { viewer: 0, steward: 1, admin: 2 };

/**
 * The shape erserver mints an org name in — enforced on the provisioning path by
 * `api/admin/orgs` and re-exported from here so the READ paths share one
 * definition.
 */
export const ORG_SLUG = /^[a-z0-9][a-z0-9_-]*$/;

/** Longest org name the provisioning schema accepts. */
const ORG_MAX_LENGTH = 128;

/**
 * Reject an org name that is not a slug, before it reaches a URL.
 *
 * Every other path parameter is `encodeURIComponent`-ed at its call site; `org`
 * never was, at any of the ~46 tenant routes that interpolate it into
 * `/v1/orgs/${org}/…`. `new URL()` resolves `..` segments, so a name containing
 * them re-points the upstream request at a different path — with the BFF's
 * privileged credential attached.
 *
 * Membership lookup already made that unreachable for tenant users: a name that
 * matches no membership 404s. The gap was the super-admin branch below, which
 * returns access for ANY string. A super admin can already reach every org, so
 * this is not privilege escalation — but it kept a traversal primitive alive
 * behind the operator token, and it meant the audit trail could record a request
 * for an org that does not exist. Both are cheap to close here, once, rather
 * than at 46 call sites.
 *
 * Exported because the four `api/admin/orgs/[org]/…` routes do NOT come through
 * requireOrgAccess — they authorize with requireSuperAdmin and call
 * `operatorFetch` with the same raw interpolation — so they call this directly.
 * operatorFetch cannot do it for them: it receives a whole path and has no way
 * to know which segment was an org name.
 */
export function requireOrgSlug(org: string): void {
  if (org.length > ORG_MAX_LENGTH || !ORG_SLUG.test(org)) {
    // 404, not 422: the same answer a non-member gets, so a malformed probe
    // learns no more than an unauthorized one.
    throw new BffFailure(404, "not_found", "not found");
  }
}

export class BffFailure extends Error {
  readonly body: BffError;
  readonly status: number;

  constructor(status: number, code: BffErrorCode, message: string, extra?: Partial<BffError>) {
    super(message);
    this.status = status;
    this.body = { code, message, ...extra };
  }

  response(): NextResponse {
    return NextResponse.json(this.body, { status: this.status });
  }
}

export async function requireIdentity(): Promise<Identity> {
  const identity = await identityFromCookies();
  if (!identity) throw new BffFailure(401, "unauthorized", "sign in to continue");
  return identity;
}

export interface EffectiveOrgAccess {
  identity: Identity;
  org: string;
  role: Membership["role"];
  /** True when acting through the operator token (super admin without membership). */
  viaOperator: boolean;
}

/** Resolve what the acting user may do inside `org`. */
export async function requireOrgAccess(
  org: string,
  minimum: Membership["role"],
): Promise<EffectiveOrgAccess> {
  // Identity first, so a stale session still gets 401 rather than having a
  // malformed org name answer 404 and hide that the session expired.
  const identity = await requireIdentity();
  requireOrgSlug(org);

  // An active impersonation REPLACES the super admin's powers entirely: the
  // impersonated role is enforced with the org's real stored key, and every
  // other org answers 404 — "view as tenant" means seeing exactly what they see.
  if (identity.impersonating && identity.user.isSuperAdmin) {
    if (identity.impersonating.org !== org) {
      throw new BffFailure(404, "not_found", "not found");
    }
    if (ROLE_RANK[identity.impersonating.role] < ROLE_RANK[minimum]) {
      throw new BffFailure(403, "forbidden", `requires the ${minimum} role in ${org}`);
    }
    return { identity, org, role: identity.impersonating.role, viaOperator: false };
  }

  const membership = identity.memberships.find((m) => m.org === org);
  if (membership) {
    if (ROLE_RANK[membership.role] < ROLE_RANK[minimum]) {
      throw new BffFailure(403, "forbidden", `requires the ${minimum} role in ${org}`);
    }
    return { identity, org, role: membership.role, viaOperator: false };
  }
  if (identity.user.isSuperAdmin) {
    return { identity, org, role: "admin", viaOperator: true };
  }
  // Non-members learn nothing about which orgs exist.
  throw new BffFailure(404, "not_found", "not found");
}

function mapUpstreamError(status: number, detail: string): BffFailure {
  const table: Record<number, [BffErrorCode, number]> = {
    401: ["internal", 502], // our stored credential was rejected — a BFF fault, not the user's
    403: ["forbidden", 403],
    404: ["not_found", 404],
    409: [detail.includes("writer lock") ? "writer_locked" : "org_not_active", 409],
    413: ["payload_too_large", 413],
    422: ["validation", 422],
    503: ["lake_unavailable", 503],
  };
  const [code, ours] = table[status] ?? ["internal", 502];
  return new BffFailure(ours, code, detail || `upstream returned ${String(status)}`);
}

export interface ForwardOptions {
  method?: string;
  /** Minimum erserver role the call needs; also selects the stored key. */
  role?: Membership["role"];
  searchParams?: URLSearchParams;
  body?: unknown;
  idempotencyKey?: string;
}

/** Call erserver as the effective identity; returns the raw `Response` once the
 * status is known to be OK. Shared by `forward` (JSON) and `forwardStream`
 * (binary pass-through) so auth, credential selection and error mapping live in
 * one place. On a non-OK status the upstream `detail` is mapped to a BffFailure. */
async function rawFetch(
  access: EffectiveOrgAccess,
  path: string,
  options: ForwardOptions,
): Promise<Response> {
  const role = options.role ?? "viewer";
  if (ROLE_RANK[access.role] < ROLE_RANK[role]) {
    throw new BffFailure(403, "forbidden", `requires the ${role} role in ${access.org}`);
  }
  const { credentialForAccess } = await import("./credentials");
  const bearer = await credentialForAccess(access, role);
  const url = new URL(path, env().ERSERVER_BASE_URL);
  if (options.searchParams) url.search = options.searchParams.toString();
  const headers: Record<string, string> = {
    authorization: `Bearer ${bearer}`,
    "x-acting-user": access.identity.user.email,
  };
  if (options.body !== undefined) headers["content-type"] = "application/json";
  if (options.idempotencyKey) headers["idempotency-key"] = options.idempotencyKey;

  let response: Response;
  try {
    response = await fetch(url, {
      method: options.method ?? "GET",
      headers,
      body: options.body === undefined ? null : JSON.stringify(options.body),
      cache: "no-store",
    });
  } catch {
    throw new BffFailure(502, "internal", "the control plane is unreachable");
  }
  if (!response.ok) {
    let detail = "";
    try {
      const parsed: unknown = await response.json();
      if (typeof parsed === "object" && parsed !== null && "detail" in parsed) {
        detail = String(parsed.detail);
      }
    } catch {
      // non-JSON upstream error body; the status is enough
    }
    throw mapUpstreamError(response.status, detail);
  }
  const method = options.method ?? "GET";
  if (method !== "GET" && access.identity.impersonating) {
    // Dual-identity trail: erserver's audit_log records "user:<email> via key:…";
    // this row records that the person was a super admin acting as the tenant.
    const { writeAudit } = await import("./audit");
    await writeAudit(access.identity, "impersonated.mutation", { method, path }, access.org);
  }
  return response;
}

/** Call erserver as the effective identity; returns the parsed JSON body. */
export async function forward<T>(
  access: EffectiveOrgAccess,
  path: string,
  options: ForwardOptions = {},
): Promise<T> {
  const response = await rawFetch(access, path, options);
  return (await response.json()) as T;
}

/** An Arrow data-lane pass-through: the upstream body as a byte stream plus the
 * content type and the snapshot / next-cursor headers the read API carries. */
export interface StreamResponse {
  body: ReadableStream<Uint8Array> | null;
  contentType: string;
  snapshot: string | null;
  nextCursor: string | null;
}

/** Call erserver and pass its body through untouched (no JSON decode) — for the
 * binary Arrow-IPC data lane. The route re-emits `body` with the preserved
 * headers so the browser decodes the stream in place. */
export async function forwardStream(
  access: EffectiveOrgAccess,
  path: string,
  options: ForwardOptions = {},
): Promise<StreamResponse> {
  const response = await rawFetch(access, path, options);
  return {
    body: response.body,
    contentType: response.headers.get("content-type") ?? "application/vnd.apache.arrow.stream",
    snapshot: response.headers.get("x-er-snapshot"),
    nextCursor: response.headers.get("x-er-next-cursor"),
  };
}

/** Wrap a route handler body so every BffFailure becomes its envelope response. */
export async function respond(run: () => Promise<NextResponse>): Promise<NextResponse> {
  try {
    return await run();
  } catch (error) {
    if (error instanceof BffFailure) return error.response();
    console.error("bff: unhandled failure", error);
    return new BffFailure(500, "internal", "internal error").response();
  }
}
