// @vitest-environment node
//
// PGlite, like every other database-backed unit file here.
import { beforeEach, describe, expect, it } from "vitest";

/**
 * The usage ledger (O12b).
 *
 * Two properties decide whether this is safe to add to six hot paths:
 *
 * 1. **`meter` never throws.** A ledger write that fails must not fail the
 *    feature it measures — a tenant whose assistant stops answering because
 *    metering is down has been given a worse product in exchange for a number
 *    nobody asked for. The consequence is stated plainly: the ledger is a FLOOR
 *    on usage, not an authoritative count, and must never be used as one.
 * 2. **`tokensFrom` tolerates whatever the SDK returns.** It is parsing a field
 *    from a third party on a path that must not break.
 */

process.env.ERWEB_DATABASE_URL = "pglite://memory";
process.env.ERWEB_SESSION_SECRET = "test-session-secret-test-session-secret";
process.env.ERWEB_CREDENTIAL_KEY = "3q2+7wEirykuXBXRO26AY9nbZAqAnDzws76jd2GxwkE=";
process.env.ERSERVER_OPERATOR_TOKEN = "test-operator-token";
delete process.env.ERWEB_TEST_FIXTURES;

const { db, schema } = await import("@/lib/db");
const usage = await import("@/lib/usage/record");

const ORG = "acme-dev";

beforeEach(async () => {
  const database = await db();
  await database.delete(schema.usageEvents);
});

describe("reading the SDK's usage block", () => {
  it("sums input and output", () => {
    // Summed rather than kept apart: the panel answers "how much", and
    // splitting them invites pricing, which is the billing question this
    // deliberately does not answer.
    expect(usage.tokensFrom({ input_tokens: 100, output_tokens: 40 })).toBe(140);
  });

  it("treats a missing, null or malformed block as zero", () => {
    // Parsing a third party's field on a path that must not break.
    for (const block of [null, undefined, {}, { input_tokens: null }, { output_tokens: "40" }]) {
      expect(usage.tokensFrom(block as never), JSON.stringify(block)).toBe(0);
    }
  });

  it("refuses a negative or non-finite total rather than recording it", () => {
    expect(usage.tokensFrom({ input_tokens: -5, output_tokens: 0 })).toBe(0);
    expect(usage.tokensFrom({ input_tokens: Number.NaN, output_tokens: 1 })).toBe(0);
  });
});

describe("recording usage", () => {
  it("writes one row per metered act", async () => {
    await usage.meter(ORG, "ai_assistant", 140, "claude-opus-5");
    const database = await db();
    const rows = await database.select().from(schema.usageEvents);
    expect(rows).toHaveLength(1);
    expect(rows[0]?.kind).toBe("ai_assistant");
    expect(rows[0]?.units).toBe(140);
    expect(rows[0]?.unit).toBe("tokens");
    expect(rows[0]?.detail).toBe("claude-opus-5");
  });

  it("records nothing for a zero or negative measurement", async () => {
    // A call that consumed nothing is not an event; a row of zero would make
    // the event COUNT wrong while leaving the total right.
    await usage.meter(ORG, "ai_enrich", 0);
    await usage.meter(ORG, "ai_enrich", -10);
    const database = await db();
    expect(await database.select().from(schema.usageEvents)).toHaveLength(0);
  });

  it("APPENDS — the same act twice is two rows, not an update", async () => {
    // A usage ledger that can be rewritten cannot answer the question it
    // exists for.
    await usage.meter(ORG, "ai_assistant", 100);
    await usage.meter(ORG, "ai_assistant", 100);
    const database = await db();
    expect(await database.select().from(schema.usageEvents)).toHaveLength(2);
  });

  it("never throws, whatever happens underneath", async () => {
    // THE property that makes it safe on six hot paths. An invalid kind is the
    // cheapest way to provoke a write failure without breaking the harness.
    await expect(usage.meter(ORG, "not_a_kind" as never, 10)).resolves.toBeUndefined();
  });
});

describe("summing usage for a window", () => {
  beforeEach(async () => {
    await usage.meter(ORG, "ai_assistant", 100, "claude-opus-5");
    await usage.meter(ORG, "ai_assistant", 40, "claude-opus-5");
    await usage.meter(ORG, "ai_enrich", 25, "claude-opus-5");
    await usage.meter("globex-dev", "ai_assistant", 9_999, "claude-opus-5");
  });

  const hourAgo = () => new Date(Date.now() - 60 * 60 * 1000);

  it("groups by kind and sums the units", async () => {
    const rows = await usage.usageSince(ORG, hourAgo());
    const assistant = rows.find((row) => row.kind === "ai_assistant");
    expect(assistant?.units).toBe(140);
    expect(assistant?.events).toBe(2);
  });

  it("never counts another tenant's usage", async () => {
    // The same leak shape V6's recommendation read had: without the org
    // predicate this reports globex's 9,999 tokens on acme's panel.
    const rows = await usage.usageSince(ORG, hourAgo());
    expect(rows.reduce((total, row) => total + row.units, 0)).toBe(165);
  });

  it("orders by volume, so the thing to look at is first", async () => {
    const rows = await usage.usageSince(ORG, hourAgo());
    expect(rows[0]?.kind).toBe("ai_assistant");
  });

  it("excludes anything before the cutoff", async () => {
    const future = new Date(Date.now() + 60 * 60 * 1000);
    expect(await usage.usageSince(ORG, future)).toEqual([]);
  });

  it("returns an empty list for an org with no usage", async () => {
    expect(await usage.usageSince("quiet-dev", hourAgo())).toEqual([]);
  });

  it("returns units as a NUMBER, whatever the driver says", async () => {
    // `sum` comes back a string from some drivers and a number from others;
    // normalising at the boundary keeps the wire shape independent of which.
    const rows = await usage.usageSince(ORG, hourAgo());
    expect(typeof rows[0]?.units).toBe("number");
    expect(typeof rows[0]?.events).toBe("number");
  });
});
