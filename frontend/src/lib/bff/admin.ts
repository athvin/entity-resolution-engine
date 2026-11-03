import "server-only";

import type { Identity } from "@/lib/auth/session";
import { db, schema } from "@/lib/db";
import { seal } from "@/lib/db/crypto";
import { env } from "@/lib/env";
import { BffFailure, requireIdentity } from "./proxy";
import { upstreamFetch } from "./upstream";

/** Super-admin-only surfaces (the operator console). */
export async function requireSuperAdmin(): Promise<Identity> {
  const identity = await requireIdentity();
  if (!identity.user.isSuperAdmin) {
    // Non-operators learn nothing about the console's existence.
    throw new BffFailure(404, "not_found", "not found");
  }
  return identity;
}

/** Call erserver with the operator token (console reads/writes only). */
export async function operatorFetch<T>(
  identity: Identity,
  path: string,
  options: { method?: string; body?: unknown; searchParams?: URLSearchParams } = {},
): Promise<T> {
  let response: Response;
  try {
    response = await upstreamFetch({
      path,
      authorization: `Bearer ${env().ERSERVER_OPERATOR_TOKEN}`,
      method: options.method,
      searchParams: options.searchParams,
      json: options.body,
      actingUser: identity.user.email,
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
      // non-JSON error body
    }
    const status = response.status === 404 ? 404 : response.status === 422 ? 422 : 502;
    throw new BffFailure(
      status,
      status === 404 ? "not_found" : status === 422 ? "validation" : "internal",
      detail || `control plane returned ${String(response.status)}`,
    );
  }
  return (await response.json()) as T;
}

/**
 * Mint the BFF's three service keys for an org and seal them into the vault.
 * The one-time admin key from provisioning is never stored — these are ours.
 */
export async function mintServiceKeys(identity: Identity, org: string): Promise<void> {
  const database = await db();
  const key = Buffer.from(env().ERWEB_CREDENTIAL_KEY, "base64");
  for (const role of ["viewer", "steward", "admin"] as const) {
    const issued = await operatorFetch<{ id: string; key: string }>(
      identity,
      `/v1/orgs/${org}/api-keys`,
      { method: "POST", body: { role } },
    );
    const sealed = seal(key, org, role, issued.key);
    await database
      .insert(schema.orgCredentials)
      .values({
        org,
        role,
        keyId: issued.id,
        ciphertext: sealed.ciphertext,
        nonce: sealed.nonce,
      })
      .onConflictDoUpdate({
        target: [schema.orgCredentials.org, schema.orgCredentials.role],
        set: {
          keyId: issued.id,
          ciphertext: sealed.ciphertext,
          nonce: sealed.nonce,
          rotatedAt: new Date(),
        },
      });
  }
}

export { writeAudit } from "./audit";
