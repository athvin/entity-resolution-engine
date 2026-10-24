/**
 * Which key, which endpoint and which model an org's AI call uses — as a pure
 * function of the stored row and the deployment environment.
 *
 * Split out of `config.ts` because every rule below is invisible when wrong.
 * None of them throws: a mistake here sends a tenant's own key to the wrong
 * endpoint, or silently bills the deployment's key for a tenant that brought
 * one, and the only symptom is an upstream 401 nobody can attribute.
 */

/** The purpose decides only which deployment-default model applies. */
export type AiPurpose = "assistant" | "enrich";

/** The row in `erweb.org_ai_settings`, or null when the tenant has no row. */
export interface AiSettingsRow {
  enabled: boolean;
  model: string | null;
  /** Whether a bring-your-own key is stored. The plaintext is passed separately
   * so this module never has to hold a secret to make a decision. */
  hasKey: boolean;
}

/** The deployment-wide fallbacks, from `env()`. */
export interface AiDeployment {
  apiKey: string | undefined;
  baseUrl: string | undefined;
  assistantModel: string;
  enrichModel: string;
}

export interface AiResolved {
  ok: true;
  apiKey: string;
  /** Absent means the public Anthropic API. */
  baseUrl: string | undefined;
  model: string;
  /** Whose key is paying for this call. Surfaced so the settings page can say. */
  keySource: "org" | "deployment";
}

export interface AiUnavailable {
  ok: false;
  /** Why, in words a 503 body can carry. */
  reason: string;
}

/**
 * Resolve the effective AI configuration.
 *
 * Precedence, in the order the checks run:
 *
 * 1. **An explicit `enabled: false` wins over everything**, including a stored
 *    key. Switching AI off for a tenant has to be one toggle, not "delete the
 *    key as well" — otherwise the off state is unreachable for anyone on the
 *    deployment key.
 * 2. **No row means inherit**, not disable. See the table comment: a tenant
 *    that has never opened this page behaves exactly as it did when AI was a
 *    process-wide variable.
 * 3. **The org's key wins over the deployment's** when present.
 * 4. **The org's model overrides both surfaces' defaults** when set.
 */
export function resolveAi(
  row: AiSettingsRow | null,
  orgApiKey: string | null,
  deployment: AiDeployment,
  purpose: AiPurpose,
): AiResolved | AiUnavailable {
  if (row !== null && !row.enabled) {
    return { ok: false, reason: "AI is switched off for this tenant" };
  }

  const defaultModel = purpose === "assistant" ? deployment.assistantModel : deployment.enrichModel;
  const model = row?.model ?? defaultModel;

  if (orgApiKey !== null && orgApiKey !== "") {
    // A tenant's own key is an Anthropic key, so it must go to the public
    // Anthropic API — NOT to `ANTHROPIC_BASE_URL`. On a Bedrock deployment that
    // base URL is a Bedrock endpoint, which would reject the key with a 401
    // that looks like "the tenant typed their key wrong". The two are
    // credentials for different services and the endpoint has to follow the
    // credential.
    return { ok: true, apiKey: orgApiKey, baseUrl: undefined, model, keySource: "org" };
  }

  if (deployment.apiKey === undefined || deployment.apiKey === "") {
    // Distinguished from the switched-off case on purpose: one is a tenant
    // decision an admin can undo here, the other is a deployment that was never
    // configured and no tenant admin can fix.
    return {
      ok: false,
      reason: row?.hasKey
        ? // Unreachable through the UI today, but the row and the plaintext are
          // read independently, so a decrypt failure lands here rather than
          // pretending the deployment is unconfigured.
          "this tenant's stored AI key could not be read"
        : "AI is not configured for this deployment (ANTHROPIC_API_KEY is unset)",
    };
  }

  return {
    ok: true,
    apiKey: deployment.apiKey,
    baseUrl: deployment.baseUrl,
    model,
    keySource: "deployment",
  };
}
