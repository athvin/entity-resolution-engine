// @vitest-environment node
//
// NOT the suite's default jsdom, and this is load-bearing rather than tidiness.
// jsdom installs its own `Uint8Array` global, so a Node `Buffer` fails
// `instanceof Uint8Array` under it — and PGlite's bytea serializer tests
// exactly that, rejecting every sealed key with "Invalid input for bytea type".
// It is purely an artifact of the test environment (in the Next.js server
// runtime a Buffer is a Uint8Array, which is why the Playwright tier's fixtures
// seal credentials into PGlite without trouble), but it means ANY future test
// that writes through the vault has to declare this too.
import { beforeEach, describe, expect, it } from "vitest";

/**
 * The AI-settings store, against a real database.
 *
 * `src/lib/ai/config.ts` is the only place a tenant's model key is written,
 * read and decrypted, and the three behaviours worth proving all involve state
 * that no pure test can hold:
 *
 * - **the vault round-trip** — a key sealed on write must open on read, under
 *   the `ai` pseudo-role, and must NOT open as an erserver credential;
 * - **the three-state merge** — `previewAiConfig` has to combine a pending
 *   write with the stored row, because the PUT verifies what it is about to
 *   save and only this module can read the stored key;
 * - **`undefined` versus `null`** on a partial write, which is the difference
 *   between leaving a tenant's key alone and deleting it.
 *
 * PGlite (in-process WASM Postgres) for the reason `bff-members.test.ts` gives:
 * `db()` already applies the committed drizzle migrations against it, and
 * mocking drizzle here would test the mock rather than the upsert.
 *
 * Env is set before any import, because `env()` parses `process.env` lazily on
 * first use and caches it for the process.
 */

process.env.ERWEB_DATABASE_URL = "pglite://memory";
process.env.ERWEB_SESSION_SECRET = "test-session-secret-test-session-secret";
process.env.ERWEB_CREDENTIAL_KEY = "3q2+7wEirykuXBXRO26AY9nbZAqAnDzws76jd2GxwkE=";
process.env.ERSERVER_OPERATOR_TOKEN = "test-operator-token";
process.env.ANTHROPIC_API_KEY = "deployment-key";
process.env.ANTHROPIC_BASE_URL = "https://bedrock.example/anthropic";
process.env.ERWEB_ASSISTANT_MODEL = "claude-opus-5";
process.env.ERWEB_ENRICH_MODEL = "claude-haiku-4-5-20251001";
delete process.env.ERWEB_TEST_FIXTURES;

const { db, schema } = await import("@/lib/db");
const { open } = await import("@/lib/db/crypto");
const ai = await import("@/lib/ai/config");

const ORG = "acme-dev";
const BY = "admin@acme.dev";

beforeEach(async () => {
  const database = await db();
  await database.delete(schema.orgAiSettings);
});

describe("the no-row default", () => {
  it("reports an unconfigured tenant as inheriting an enabled deployment", async () => {
    const view = await ai.readAiSettings(ORG);
    expect(view.configured).toBe(false);
    expect(view.enabled).toBe(true);
    expect(view.deployment_has_key).toBe(true);
    expect(view.key_set).toBe(false);
    expect(view.model).toBeNull();
  });

  it("resolves to the deployment key, endpoint and per-surface model", async () => {
    const assistant = await ai.resolveAiConfig(ORG, "assistant");
    expect(assistant.ok).toBe(true);
    if (assistant.ok) {
      expect(assistant.apiKey).toBe("deployment-key");
      expect(assistant.baseUrl).toBe("https://bedrock.example/anthropic");
      expect(assistant.model).toBe("claude-opus-5");
    }
    const enrich = await ai.resolveAiConfig(ORG, "enrich");
    expect(enrich.ok && enrich.model).toBe("claude-haiku-4-5-20251001");
  });
});

describe("the vault round-trip", () => {
  it("seals on write and opens on read", async () => {
    await ai.writeAiSettings(ORG, BY, { enabled: true, apiKey: "sk-ant-tenant" });
    const resolved = await ai.resolveAiConfig(ORG, "assistant");
    expect(resolved.ok).toBe(true);
    if (resolved.ok) {
      expect(resolved.apiKey).toBe("sk-ant-tenant");
      expect(resolved.keySource).toBe("org");
      // The rule from `resolve.ts`: a tenant key goes to the public API, not to
      // this deployment's Bedrock endpoint.
      expect(resolved.baseUrl).toBeUndefined();
    }
  });

  it("stores ciphertext, not the key", async () => {
    await ai.writeAiSettings(ORG, BY, { enabled: true, apiKey: "sk-ant-tenant" });
    const database = await db();
    const [row] = await database.select().from(schema.orgAiSettings);
    expect(row?.ciphertext).toBeDefined();
    // The plaintext must not appear anywhere in the stored bytes.
    expect(row?.ciphertext?.toString("utf8")).not.toContain("sk-ant-tenant");
  });

  it("binds the AAD to the `ai` pseudo-role, so a credential row cannot cross over", async () => {
    await ai.writeAiSettings(ORG, BY, { enabled: true, apiKey: "sk-ant-tenant" });
    const database = await db();
    const [row] = await database.select().from(schema.orgAiSettings);
    const key = Buffer.from(process.env.ERWEB_CREDENTIAL_KEY ?? "", "base64");
    const sealed = {
      ciphertext: row?.ciphertext ?? Buffer.alloc(0),
      nonce: row?.nonce ?? Buffer.alloc(0),
    };
    // Opens as `ai`…
    expect(open(key, ORG, "ai", sealed)).toBe("sk-ant-tenant");
    // …and refuses under every real erserver role, which is the whole point of
    // choosing a pseudo-role outside `viewer|steward|admin`.
    for (const role of ["viewer", "steward", "admin"]) {
      expect(() => open(key, ORG, role, sealed), role).toThrow();
    }
    // And under another org, as the vault's own test asserts.
    expect(() => open(key, "globex", "ai", sealed)).toThrow();
  });

  it("never returns the key through the settings view", async () => {
    await ai.writeAiSettings(ORG, BY, { enabled: true, apiKey: "sk-ant-tenant" });
    const view = await ai.readAiSettings(ORG);
    expect(view.key_set).toBe(true);
    expect(view.key_set_at).not.toBeNull();
    // There is no field that could carry it, and no value in the object that is
    // the key — asserted over the serialized form so a future field is caught.
    expect(JSON.stringify(view)).not.toContain("sk-ant-tenant");
  });
});

describe("a partial write leaves what it does not mention", () => {
  beforeEach(async () => {
    await ai.writeAiSettings(ORG, BY, {
      enabled: true,
      model: "claude-sonnet-5",
      apiKey: "sk-ant-tenant",
    });
  });

  it("keeps the stored key when `apiKey` is undefined", async () => {
    // THE data-loss case: flipping the toggle must not delete the key.
    await ai.writeAiSettings(ORG, BY, { enabled: false });
    const view = await ai.readAiSettings(ORG);
    expect(view.key_set).toBe(true);
    expect(view.enabled).toBe(false);
  });

  it("keeps the stored model when `model` is undefined", async () => {
    await ai.writeAiSettings(ORG, BY, { enabled: false });
    expect((await ai.readAiSettings(ORG)).model).toBe("claude-sonnet-5");
  });

  it("deletes the key on an explicit null, and falls back to the deployment", async () => {
    await ai.writeAiSettings(ORG, BY, { enabled: true, apiKey: null });
    const view = await ai.readAiSettings(ORG);
    expect(view.key_set).toBe(false);
    expect(view.key_set_at).toBeNull();
    const resolved = await ai.resolveAiConfig(ORG, "assistant");
    expect(resolved.ok && resolved.keySource).toBe("deployment");
  });

  it("clears the model on an explicit null, back to the per-surface default", async () => {
    await ai.writeAiSettings(ORG, BY, { enabled: true, model: null });
    expect((await ai.readAiSettings(ORG)).model).toBeNull();
    const enrich = await ai.resolveAiConfig(ORG, "enrich");
    expect(enrich.ok && enrich.model).toBe("claude-haiku-4-5-20251001");
  });

  it("upserts rather than duplicating, and re-stamps the author", async () => {
    await ai.writeAiSettings(ORG, "someone-else@acme.dev", { enabled: false });
    const database = await db();
    const rows = await database.select().from(schema.orgAiSettings);
    expect(rows).toHaveLength(1);
    expect(rows[0]?.updatedBy).toBe("someone-else@acme.dev");
  });
});

describe("previewAiConfig resolves a write without performing it", () => {
  it("writes nothing", async () => {
    await ai.previewAiConfig(ORG, { enabled: true, apiKey: "sk-ant-new" }, "assistant");
    const database = await db();
    expect(await database.select().from(schema.orgAiSettings)).toHaveLength(0);
  });

  it("verifies the SUBMITTED key, not the stored one", async () => {
    await ai.writeAiSettings(ORG, BY, { enabled: true, apiKey: "sk-ant-old" });
    const preview = await ai.previewAiConfig(
      ORG,
      { enabled: true, apiKey: "sk-ant-new" },
      "assistant",
    );
    expect(preview.ok).toBe(true);
    if (preview.ok) expect(preview.apiKey).toBe("sk-ant-new");
  });

  it("verifies the STORED key when the write omits one", async () => {
    // The case that makes this function necessary: a PUT changing only the
    // model must be verified against the key the call will really use, which
    // is not in the request at all.
    await ai.writeAiSettings(ORG, BY, { enabled: true, apiKey: "sk-ant-old" });
    const preview = await ai.previewAiConfig(
      ORG,
      { enabled: true, model: "claude-sonnet-5" },
      "assistant",
    );
    expect(preview.ok).toBe(true);
    if (preview.ok) {
      expect(preview.apiKey).toBe("sk-ant-old");
      expect(preview.model).toBe("claude-sonnet-5");
      expect(preview.baseUrl).toBeUndefined();
    }
  });

  it("falls back to the deployment when the write clears the key", async () => {
    await ai.writeAiSettings(ORG, BY, { enabled: true, apiKey: "sk-ant-old" });
    const preview = await ai.previewAiConfig(ORG, { enabled: true, apiKey: null }, "assistant");
    expect(preview.ok).toBe(true);
    if (preview.ok) {
      expect(preview.apiKey).toBe("deployment-key");
      // And the endpoint comes back with it, since it is the deployment's key.
      expect(preview.baseUrl).toBe("https://bedrock.example/anthropic");
    }
  });

  it("inherits the stored model when the write omits one", async () => {
    await ai.writeAiSettings(ORG, BY, { enabled: true, model: "claude-sonnet-5" });
    const preview = await ai.previewAiConfig(ORG, { enabled: true }, "enrich");
    expect(preview.ok && preview.model).toBe("claude-sonnet-5");
  });
});
