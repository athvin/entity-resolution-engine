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
  const identity = await requireIdentity();

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

/** Call erserver as the effective identity; returns the parsed JSON body. */
export async function forward<T>(
  access: EffectiveOrgAccess,
  path: string,
  options: ForwardOptions = {},
): Promise<T> {
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
  return (await response.json()) as T;
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
