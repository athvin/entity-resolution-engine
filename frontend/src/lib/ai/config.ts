import "server-only";
import { eq } from "drizzle-orm";

import { db, schema } from "@/lib/db";
import { open, seal } from "@/lib/db/crypto";
import { env } from "@/lib/env";
import {
  resolveAi,
  type AiPurpose,
  type AiResolved,
  type AiSettingsRow,
  type AiUnavailable,
} from "./resolve";

/**
 * The pseudo-role the BYO key is sealed under.
 *
 * The vault's AAD is `${org}\0${role}`, and the real roles are
 * `viewer|steward|admin`, so `ai` cannot collide with any of them: an erserver
 * credential row copied into `org_ai_settings` fails to decrypt rather than
 * being used as a model key, and the reverse holds too.
 */
const VAULT_ROLE = "ai";

/** What the settings surface may know. Deliberately has no `apiKey` field. */
export interface AiSettingsView {
  /** The effective state, with the no-row case already resolved to a boolean. */
  enabled: boolean;
  /** Null when inheriting the deployment default for each surface. */
  model: string | null;
  /** Whether a bring-your-own key is stored — never the key, nor a hint of it. */
  key_set: boolean;
  key_set_at: string | null;
  /** Whether this tenant has ever been configured, as opposed to inheriting. */
  configured: boolean;
  updated_by: string | null;
  updated_at: string | null;
  /** True when the deployment itself has no key, so the toggle cannot help. */
  deployment_has_key: boolean;
}

async function readRow(org: string) {
  const database = await db();
  const rows = await database
    .select()
    .from(schema.orgAiSettings)
    .where(eq(schema.orgAiSettings.org, org))
    .limit(1);
  return rows[0] ?? null;
}

function credentialKey(): Buffer {
  return Buffer.from(env().ERWEB_CREDENTIAL_KEY, "base64");
}

type StoredRow = Awaited<ReturnType<typeof readRow>>;

/**
 * The stored key's plaintext, or null when there is none or it will not open.
 *
 * A decrypt failure is swallowed rather than thrown: a tenant whose key was
 * sealed under a rotated `ERWEB_CREDENTIAL_KEY` should get the same clean 503
 * as an unconfigured one, not a 500 from a route that was asked a perfectly
 * reasonable question. `resolveAi` reports it as unreadable rather than absent,
 * because `hasKey` and the plaintext are passed separately.
 */
function decrypt(org: string, row: StoredRow): string | null {
  if (row?.ciphertext == null || row.nonce == null) return null;
  try {
    return open(credentialKey(), org, VAULT_ROLE, {
      ciphertext: row.ciphertext,
      nonce: row.nonce,
    });
  } catch {
    return null;
  }
}

function asSettings(row: StoredRow): AiSettingsRow | null {
  return row === null
    ? null
    : { enabled: row.enabled, model: row.model, hasKey: row.ciphertext !== null };
}

/** The effective configuration for one call, or why there isn't one. */
export async function resolveAiConfig(
  org: string,
  purpose: AiPurpose,
): Promise<AiResolved | AiUnavailable> {
  const row = await readRow(org);
  return resolveAi(asSettings(row), decrypt(org, row), deployment(), purpose);
}

/**
 * What `writeAiSettings(org, _, write)` would make effective, resolved without
 * writing anything.
 *
 * This exists so the PUT handler can verify the configuration it is about to
 * save with one call and no branching. The merge has to happen here because
 * only this module can read the stored key: verifying the *submitted* fields
 * alone would check a new model id against the deployment key while the
 * tenant's own stored key is what the call will really use.
 */
export async function previewAiConfig(
  org: string,
  write: AiSettingsWrite,
  purpose: AiPurpose,
): Promise<AiResolved | AiUnavailable> {
  const row = await readRow(org);
  const storedKey = decrypt(org, row);
  const key = write.apiKey === undefined ? storedKey : write.apiKey;
  const merged: AiSettingsRow = {
    enabled: write.enabled,
    model: write.model === undefined ? (row?.model ?? null) : write.model,
    hasKey: key !== null,
  };
  return resolveAi(merged, key, deployment(), purpose);
}

function deployment() {
  const settings = env();
  return {
    apiKey: settings.ANTHROPIC_API_KEY,
    baseUrl: settings.ANTHROPIC_BASE_URL,
    assistantModel: settings.ERWEB_ASSISTANT_MODEL,
    enrichModel: settings.ERWEB_ENRICH_MODEL,
  };
}

/** The settings surface's read. Resolves the no-row case so the UI shows the
 * state that is actually in force rather than an empty form. */
export async function readAiSettings(org: string): Promise<AiSettingsView> {
  const row = await readRow(org);
  const deploymentHasKey = (env().ANTHROPIC_API_KEY ?? "") !== "";
  return {
    enabled: row?.enabled ?? deploymentHasKey,
    model: row?.model ?? null,
    key_set: row?.ciphertext != null,
    key_set_at: row?.keySetAt?.toISOString() ?? null,
    configured: row !== null,
    updated_by: row?.updatedBy ?? null,
    // No `?.` after `updatedAt`: the column is NOT NULL, unlike `keySetAt`.
    updated_at: row?.updatedAt.toISOString() ?? null,
    deployment_has_key: deploymentHasKey,
  };
}

export interface AiSettingsWrite {
  enabled: boolean;
  /** `undefined` leaves the stored value; `null` clears it to the default. */
  model?: string | null | undefined;
  /** `undefined` leaves the stored key; `null` deletes it. */
  apiKey?: string | null | undefined;
}

/**
 * Upsert the row.
 *
 * `undefined` means "leave alone" and `null` means "clear", which is why the
 * write takes the two separately rather than letting absence mean clear: a PUT
 * that omitted `api_key` would otherwise silently delete a working key every
 * time an admin flipped the toggle.
 */
export async function writeAiSettings(
  org: string,
  by: string,
  write: AiSettingsWrite,
): Promise<void> {
  const database = await db();
  const existing = await readRow(org);

  let ciphertext = existing?.ciphertext ?? null;
  let nonce = existing?.nonce ?? null;
  let keySetAt = existing?.keySetAt ?? null;
  if (write.apiKey === null) {
    ciphertext = null;
    nonce = null;
    keySetAt = null;
  } else if (write.apiKey !== undefined) {
    const sealed = seal(credentialKey(), org, VAULT_ROLE, write.apiKey);
    ciphertext = sealed.ciphertext;
    nonce = sealed.nonce;
    keySetAt = new Date();
  }

  const model = write.model === undefined ? (existing?.model ?? null) : write.model;
  const values = {
    org,
    enabled: write.enabled,
    model,
    ciphertext,
    nonce,
    keySetAt,
    updatedBy: by,
    updatedAt: new Date(),
  };
  await database
    .insert(schema.orgAiSettings)
    .values(values)
    .onConflictDoUpdate({ target: schema.orgAiSettings.org, set: values });
}
