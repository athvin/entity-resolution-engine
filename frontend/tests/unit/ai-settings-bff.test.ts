/**
 * The AI-settings write body.
 *
 * The thing under test is the three-state encoding. This endpoint has to tell
 * "leave it alone" from "clear it" from "set it" for both the model and the
 * key, and the failure mode of collapsing the first two is data loss that looks
 * like a key spontaneously stopping: a PUT sent to flip the enable toggle would
 * delete the tenant's stored key on its way past.
 */
import { describe, expect, it } from "vitest";

import { parseAiSettings } from "@/lib/bff/ai-settings";

describe("the three states of a mutable field", () => {
  it("distinguishes absent from null", () => {
    const absent = parseAiSettings({ enabled: true });
    const cleared = parseAiSettings({ enabled: true, model: null, api_key: null });
    expect(absent.ok && "model" in absent.value).toBe(false);
    expect(absent.ok && "api_key" in absent.value).toBe(false);
    expect(cleared.ok && cleared.value.model).toBeNull();
    expect(cleared.ok && cleared.value.api_key).toBeNull();
  });

  it("accepts a set value for each", () => {
    const parsed = parseAiSettings({
      enabled: true,
      model: "claude-sonnet-5",
      api_key: "sk-ant-abcdefgh",
    });
    expect(parsed.ok).toBe(true);
    if (parsed.ok) {
      expect(parsed.value.model).toBe("claude-sonnet-5");
      expect(parsed.value.api_key).toBe("sk-ant-abcdefgh");
    }
  });

  it("trims both, so whitespace is not a value", () => {
    // An untrimmed "  " would be stored as a model id and then 404 on every
    // call, which is the silence the trim exists to prevent.
    const parsed = parseAiSettings({
      enabled: true,
      model: "  claude-sonnet-5  ",
      api_key: "  sk-ant-abcdefgh  ",
    });
    expect(parsed.ok).toBe(true);
    if (parsed.ok) {
      expect(parsed.value.model).toBe("claude-sonnet-5");
      expect(parsed.value.api_key).toBe("sk-ant-abcdefgh");
    }
  });

  it("refuses a whitespace-only model rather than storing it", () => {
    // Clearing is `null`. An empty string is a mistake, and accepting it would
    // store an unusable model id that reads as a deliberate choice.
    expect(parseAiSettings({ enabled: true, model: "   " }).ok).toBe(false);
  });
});

describe("enabled is required", () => {
  it("refuses a body without it", () => {
    // Not defaulted: a PUT whose `enabled` was lost in transit would otherwise
    // silently switch a tenant's AI off (or on) as a side effect of a model
    // change.
    const parsed = parseAiSettings({ model: "claude-sonnet-5" });
    expect(parsed.ok).toBe(false);
    if (!parsed.ok) expect(parsed.reason).toContain("enabled");
  });

  it("refuses a non-boolean", () => {
    for (const enabled of ["true", 1, null]) {
      expect(parseAiSettings({ enabled }).ok, `enabled ${JSON.stringify(enabled)}`).toBe(false);
    }
  });

  it("refuses a non-object body", () => {
    for (const body of [null, "enabled", 7, []]) {
      expect(parseAiSettings(body).ok, JSON.stringify(body)).toBe(false);
    }
  });
});

describe("bounds", () => {
  it("refuses a key too short to be one", () => {
    expect(parseAiSettings({ enabled: true, api_key: "sk-ant" }).ok).toBe(false);
  });

  it("refuses values past the column's length", () => {
    expect(parseAiSettings({ enabled: true, model: "x".repeat(201) }).ok).toBe(false);
    expect(parseAiSettings({ enabled: true, model: "x".repeat(200) }).ok).toBe(true);
    expect(parseAiSettings({ enabled: true, api_key: "x".repeat(501) }).ok).toBe(false);
  });

  it("names the offending field rather than a generic message", () => {
    const parsed = parseAiSettings({ enabled: true, api_key: "sk" });
    expect(parsed.ok).toBe(false);
    if (!parsed.ok) expect(parsed.reason).toContain("api_key");
  });
});
