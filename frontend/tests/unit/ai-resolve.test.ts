/**
 * Which key, endpoint and model an org's AI call resolves to.
 *
 * Every rule here is invisible when wrong — none of them throws, and the
 * symptom of each is an upstream 401 or 404 that names nothing about the tenant
 * whose settings caused it:
 *
 * - treating a missing row as "disabled" switches AI off for every existing
 *   tenant the moment the table ships;
 * - treating a stored key as overriding the off switch makes the off state
 *   unreachable;
 * - sending a tenant's Anthropic key to `ANTHROPIC_BASE_URL` sends it to
 *   Bedrock on a Bedrock deployment, and the 401 reads as "your key is wrong";
 * - collapsing "unconfigured deployment" and "switched off" into one message
 *   tells an admin to fix something they cannot reach.
 */
import { describe, expect, it } from "vitest";

import { resolveAi, type AiDeployment, type AiSettingsRow } from "@/lib/ai/resolve";

/** A deployment with a key and a Bedrock-style base URL. */
const BEDROCK: AiDeployment = {
  apiKey: "deployment-key",
  baseUrl: "https://bedrock.example/anthropic",
  assistantModel: "claude-opus-5",
  enrichModel: "claude-haiku-4-5-20251001",
};

/** A deployment that was never given a key. */
const UNCONFIGURED: AiDeployment = { ...BEDROCK, apiKey: undefined };

function row(over: Partial<AiSettingsRow> = {}): AiSettingsRow {
  return { enabled: true, model: null, hasKey: false, ...over };
}

describe("no row means inherit, not disable", () => {
  it("uses the deployment key and endpoint when the tenant has no row", () => {
    const resolved = resolveAi(null, null, BEDROCK, "assistant");
    expect(resolved).toEqual({
      ok: true,
      apiKey: "deployment-key",
      baseUrl: "https://bedrock.example/anthropic",
      model: "claude-opus-5",
      keySource: "deployment",
    });
  });

  it("is unavailable only because the deployment has no key", () => {
    const resolved = resolveAi(null, null, UNCONFIGURED, "assistant");
    expect(resolved.ok).toBe(false);
    if (!resolved.ok) {
      // Names the deployment, not the tenant: a tenant admin reading this must
      // not go looking for a setting of their own to change.
      expect(resolved.reason).toContain("deployment");
    }
  });
});

describe("the off switch wins", () => {
  it("refuses when disabled even though the deployment has a key", () => {
    const resolved = resolveAi(row({ enabled: false }), null, BEDROCK, "assistant");
    expect(resolved.ok).toBe(false);
    if (!resolved.ok) expect(resolved.reason).toContain("switched off");
  });

  it("refuses when disabled even though the tenant brought a key", () => {
    // The case that makes the off state reachable: if a stored key overrode the
    // toggle, switching AI off would also require deleting the key.
    const resolved = resolveAi(row({ enabled: false, hasKey: true }), "org-key", BEDROCK, "enrich");
    expect(resolved.ok).toBe(false);
    if (!resolved.ok) expect(resolved.reason).toContain("switched off");
  });
});

describe("whose key pays", () => {
  it("prefers the tenant's key over the deployment's", () => {
    const resolved = resolveAi(row({ hasKey: true }), "org-key", BEDROCK, "assistant");
    expect(resolved.ok).toBe(true);
    if (resolved.ok) {
      expect(resolved.apiKey).toBe("org-key");
      expect(resolved.keySource).toBe("org");
    }
  });

  it("sends a tenant key to the PUBLIC API, never to the deployment's base URL", () => {
    // The rule that is pure correctness rather than preference: an Anthropic
    // key and a Bedrock endpoint are credentials and server for two different
    // services, so the endpoint has to follow the credential.
    const resolved = resolveAi(row({ hasKey: true }), "org-key", BEDROCK, "assistant");
    expect(resolved.ok).toBe(true);
    if (resolved.ok) expect(resolved.baseUrl).toBeUndefined();
  });

  it("keeps the deployment's base URL when using the deployment's key", () => {
    const resolved = resolveAi(row(), null, BEDROCK, "assistant");
    expect(resolved.ok).toBe(true);
    if (resolved.ok) expect(resolved.baseUrl).toBe("https://bedrock.example/anthropic");
  });

  it("works on an unconfigured deployment when the tenant brought a key", () => {
    // A deployment with no key of its own is still usable by any tenant that
    // supplies one — otherwise BYO would only work where it was least needed.
    const resolved = resolveAi(row({ hasKey: true }), "org-key", UNCONFIGURED, "assistant");
    expect(resolved.ok).toBe(true);
    if (resolved.ok) expect(resolved.keySource).toBe("org");
  });

  it("reports an unreadable stored key as unreadable, not as absent", () => {
    // `hasKey` and the plaintext are passed separately precisely so this case
    // is distinguishable: a key sealed under a rotated ERWEB_CREDENTIAL_KEY
    // must not be reported as "this deployment has no key".
    const resolved = resolveAi(row({ hasKey: true }), null, UNCONFIGURED, "assistant");
    expect(resolved.ok).toBe(false);
    if (!resolved.ok) expect(resolved.reason).toContain("could not be read");
  });

  it("falls back to the deployment key when the tenant's cannot be read", () => {
    // A decrypt failure degrades to the deployment key rather than to an
    // outage: the tenant keeps working while an operator investigates.
    const resolved = resolveAi(row({ hasKey: true }), null, BEDROCK, "assistant");
    expect(resolved.ok).toBe(true);
    if (resolved.ok) expect(resolved.keySource).toBe("deployment");
  });

  it("treats an empty-string key as no key", () => {
    const resolved = resolveAi(row({ hasKey: true }), "", BEDROCK, "assistant");
    expect(resolved.ok).toBe(true);
    if (resolved.ok) expect(resolved.keySource).toBe("deployment");
  });
});

describe("which model answers", () => {
  it("uses each surface's own deployment default when the tenant sets none", () => {
    // The two defaults stay separate in env so a Bedrock cutover can move one
    // surface at a time; only a tenant override collapses them.
    const assistant = resolveAi(row(), null, BEDROCK, "assistant");
    const enrich = resolveAi(row(), null, BEDROCK, "enrich");
    expect(assistant.ok && assistant.model).toBe("claude-opus-5");
    expect(enrich.ok && enrich.model).toBe("claude-haiku-4-5-20251001");
  });

  it("applies a tenant override to BOTH surfaces", () => {
    const over = row({ model: "claude-sonnet-5" });
    const assistant = resolveAi(over, null, BEDROCK, "assistant");
    const enrich = resolveAi(over, null, BEDROCK, "enrich");
    expect(assistant.ok && assistant.model).toBe("claude-sonnet-5");
    expect(enrich.ok && enrich.model).toBe("claude-sonnet-5");
  });

  it("overrides the model independently of whose key pays", () => {
    const resolved = resolveAi(
      row({ model: "claude-sonnet-5", hasKey: true }),
      "org-key",
      BEDROCK,
      "enrich",
    );
    expect(resolved.ok).toBe(true);
    if (resolved.ok) {
      expect(resolved.model).toBe("claude-sonnet-5");
      expect(resolved.keySource).toBe("org");
    }
  });
});
