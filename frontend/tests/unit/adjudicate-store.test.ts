// @vitest-environment node
//
// Node rather than the suite's jsdom. This file writes no bytea, so it is not
// the `Buffer`/`Uint8Array` trap `ai-store.test.ts` documents — but it does run
// PGlite, and keeping every database-backed unit file on the same environment
// means the next one that DOES seal something is not a fresh debugging session.
import { beforeEach, describe, expect, it } from "vitest";

/**
 * The recommendation store (V6), against a real database.
 *
 * Two behaviours are worth proving with real SQL rather than a mock:
 *
 *  * **replace, never append** — one row per review, because two opinions on
 *    one pair is a question the UI cannot answer. The upsert target is a
 *    composite primary key, which is exactly the kind of thing that silently
 *    becomes an insert when the target is written wrong;
 *  * **the batch read is scoped to its org AND its id list** — a recommendation
 *    leaking across tenants would be a cross-tenant disclosure of what a model
 *    said about someone else's records.
 *
 * PGlite for the reason `bff-members.test.ts` gives: `db()` already applies the
 * committed drizzle migrations against it, and mocking drizzle would test the
 * mock rather than the upsert.
 */

process.env.ERWEB_DATABASE_URL = "pglite://memory";
process.env.ERWEB_SESSION_SECRET = "test-session-secret-test-session-secret";
process.env.ERWEB_CREDENTIAL_KEY = "3q2+7wEirykuXBXRO26AY9nbZAqAnDzws76jd2GxwkE=";
process.env.ERSERVER_OPERATOR_TOKEN = "test-operator-token";
delete process.env.ERWEB_TEST_FIXTURES;

const { db, schema } = await import("@/lib/db");
const store = await import("@/lib/adjudicate/store");
type Recommendation = import("@/lib/adjudicate/propose").Recommendation;

const ORG = "acme-dev";
const META = { model: "claude-opus-5", matchProbability: 0.81, requestedBy: "steward@acme.dev" };

function rec(over: Partial<Recommendation> = {}): Recommendation {
  return {
    verdict: "match",
    confidence: 0.9,
    reasoning: "Same email and phone.",
    ...over,
  };
}

beforeEach(async () => {
  const database = await db();
  await database.delete(schema.reviewRecommendations);
});

describe("saving", () => {
  it("round-trips every field, including what the matcher thought", () => {
    // `match_probability` is stored so a later read can see what the model saw,
    // rather than re-deriving it from a score that has since moved.
    return store.saveRecommendation(ORG, "01REVIEW001", rec(), META).then(async (saved) => {
      expect(saved.verdict).toBe("match");
      expect(saved.confidence).toBe(0.9);
      expect(saved.reasoning).toBe("Same email and phone.");
      expect(saved.model).toBe("claude-opus-5");
      expect(saved.match_probability).toBe(0.81);
      expect(saved.requested_by).toBe("steward@acme.dev");
      expect(saved.review_id).toBe("01REVIEW001");
      expect(Number.isNaN(Date.parse(saved.created_at))).toBe(false);
      const found = await store.recommendationsFor(ORG, ["01REVIEW001"]);
      expect(found["01REVIEW001"]).toEqual(saved);
    });
  });

  it("REPLACES rather than appends on a re-ask", async () => {
    // The behaviour the composite PK exists for. A second row would leave the
    // inbox choosing between a stale and a fresh opinion with no rule to pick.
    await store.saveRecommendation(ORG, "01REVIEW001", rec({ verdict: "match" }), META);
    await store.saveRecommendation(
      ORG,
      "01REVIEW001",
      rec({ verdict: "no_match", reasoning: "Different birth dates." }),
      META,
    );
    const database = await db();
    const rows = await database.select().from(schema.reviewRecommendations);
    expect(rows).toHaveLength(1);
    expect(rows[0]?.verdict).toBe("no_match");
    expect(rows[0]?.reasoning).toBe("Different birth dates.");
  });

  it("keeps separate reviews separate", async () => {
    await store.saveRecommendation(ORG, "01REVIEW001", rec({ verdict: "match" }), META);
    await store.saveRecommendation(ORG, "01REVIEW002", rec({ verdict: "skip" }), META);
    const found = await store.recommendationsFor(ORG, ["01REVIEW001", "01REVIEW002"]);
    expect(found["01REVIEW001"]?.verdict).toBe("match");
    expect(found["01REVIEW002"]?.verdict).toBe("skip");
  });

  it("stores a null match_probability rather than coercing it", async () => {
    // An unscored escalation has no probability, and 0.0 would read as "the
    // matcher was certain they differ".
    const saved = await store.saveRecommendation(ORG, "01REVIEW003", rec(), {
      ...META,
      matchProbability: null,
    });
    expect(saved.match_probability).toBeNull();
  });
});

describe("the batch read", () => {
  beforeEach(async () => {
    await store.saveRecommendation(ORG, "01REVIEW001", rec(), META);
    await store.saveRecommendation(ORG, "01REVIEW002", rec({ verdict: "skip" }), META);
    await store.saveRecommendation("globex-dev", "01REVIEW001", rec({ verdict: "no_match" }), META);
  });

  it("never returns another tenant's recommendation", async () => {
    // Same review id in two orgs: without the org predicate this returns
    // globex's opinion about records acme cannot see.
    const found = await store.recommendationsFor(ORG, ["01REVIEW001"]);
    expect(found["01REVIEW001"]?.verdict).toBe("match");
    expect(Object.keys(found)).toEqual(["01REVIEW001"]);
  });

  it("returns only the ids asked for", async () => {
    const found = await store.recommendationsFor(ORG, ["01REVIEW002"]);
    expect(Object.keys(found)).toEqual(["01REVIEW002"]);
  });

  it("omits ids with no recommendation rather than inventing a placeholder", async () => {
    // The badge's `undefined` branch is "not asked yet", which is a different
    // screen from "asked and got nothing".
    const found = await store.recommendationsFor(ORG, ["01REVIEW001", "01NOTHING"]);
    expect(found["01NOTHING"]).toBeUndefined();
    expect(found["01REVIEW001"]).toBeDefined();
  });

  it("short-circuits an empty id list", async () => {
    // `inArray` with an empty list is a SQL error in some drivers and a full
    // scan in others; neither is what "nothing to look up" means.
    expect(await store.recommendationsFor(ORG, [])).toEqual({});
  });
});
