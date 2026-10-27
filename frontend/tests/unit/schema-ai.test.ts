/**
 * AI-assisted schema mapping (V8), the deterministic parts.
 *
 * **Redaction is the security property of this whole feature.**
 * `csv-profile.ts` states that the sample file never leaves the machine, and
 * V8 is the first thing that sends anything derived from it anywhere. If a
 * real value escapes in a `shapes` array, a feature that answers a question
 * about column NAMES has quietly shipped customer records to a third party —
 * and nothing in the UI would show it.
 *
 * So most of this file is one question asked many ways: can a value be read
 * back out of what we send? The rest guards `parseMappings`, where a
 * hallucinated column would be written into a source config and fail at ingest
 * time, far from the wizard that caused it.
 */
import { describe, expect, it } from "vitest";

import type { ColumnProfile } from "@/lib/domain/csv-profile";
import { mappingSystemPrompt, mappingUserContent, parseMappings } from "@/lib/schema-ai/propose";
import { redactProfile, shapeOf, type SchemaProfile } from "@/lib/schema-ai/redact";

describe("shapes carry structure, never content", () => {
  it("collapses letters and digits", () => {
    expect(shapeOf("jane.doe@acme.com")).toBe("a.a@a.a");
    expect(shapeOf("+1 (628) 555-0101")).toBe("+9 (9) 9-9");
    expect(shapeOf("1985-03-14")).toBe("9-9-9");
  });

  it("gives two different people the same shape", () => {
    // The property that makes this safe to send: a shape identifies a FORMAT,
    // and formats are not personal.
    expect(shapeOf("jane.doe@acme.com")).toBe(shapeOf("bob.smith@globex.io"));
    expect(shapeOf("Robert")).toBe(shapeOf("Jo"));
  });

  it("collapses runs, so length does not leak", () => {
    // An uncollapsed shape would carry the length of a surname, which with a
    // rare name is close to identifying.
    expect(shapeOf("Wolfeschlegelstein")).toBe("a");
    expect(shapeOf("Li")).toBe("a");
  });

  it("keeps punctuation, because that is the actual signal", () => {
    // `a@a.a` versus `a-a-a` is what tells an email column from a phone one.
    expect(shapeOf("x@y.z")).toBe("a@a.a");
    expect(shapeOf("x-y-z")).toBe("a-a-a");
    expect(shapeOf("a.b.c.d")).toBe("a.a.a.a");
  });

  it("is idempotent, which is what lets the server re-shape without cost", () => {
    // The route re-shapes rather than trusting the client. An honest client's
    // shapes must survive that unchanged or every suggestion would be garbage.
    for (const value of ["jane@acme.com", "628-555-0101", "1985-03-14", "Acme Inc."]) {
      expect(shapeOf(shapeOf(value)), value).toBe(shapeOf(value));
    }
  });

  it("bounds the shape", () => {
    expect(shapeOf("x".repeat(500)).length).toBeLessThanOrEqual(40);
    expect(shapeOf("x-".repeat(500)).length).toBe(40);
  });

  it("survives empty and whitespace without producing a shape", () => {
    expect(shapeOf("")).toBe("");
    expect(shapeOf("   ")).toBe("");
  });
});

describe("the redacted profile", () => {
  const columns: ColumnProfile[] = [
    { name: "email_address", fillRate: 0.98, samples: ["jane.doe@acme.com", "bob@globex.io"] },
    { name: "fnm", fillRate: 1, samples: ["Jane", "Bob"] },
    { name: "notes", fillRate: 0.02, samples: ["called 2026-01-04 re: renewal"] },
  ];

  it("carries no sample value anywhere in what is sent", () => {
    // THE assertion. Serialized and searched, so a future field that forwards a
    // value is caught even if nobody updates this test deliberately.
    const sent = JSON.stringify(redactProfile(columns, 400, "person"));
    for (const secret of ["jane.doe", "acme.com", "globex", "Jane", "Bob", "renewal"]) {
      expect(sent, secret).not.toContain(secret);
    }
  });

  it("keeps the column NAME, which is the question being asked", () => {
    const sent = JSON.stringify(redactProfile(columns, 400, "person"));
    expect(sent).toContain("email_address");
    expect(sent).toContain("fnm");
  });

  it("rounds the fill rate, so it is not a row count in disguise", () => {
    const profile = redactProfile([{ name: "x", fillRate: 7 / 13, samples: [] }], 13, "person");
    expect(profile.columns[0]?.fill_rate).toBe(0.54);
  });

  it("reports how many distinct values were seen, not what they were", () => {
    const profile = redactProfile(columns, 400, "person");
    expect(profile.columns[0]?.distinct_in_sample).toBe(2);
  });

  it("caps shapes per column and columns per profile", () => {
    const many: ColumnProfile[] = Array.from({ length: 200 }, (_, i) => ({
      name: `col_${String(i)}`,
      fillRate: 1,
      samples: ["a", "bb", "ccc", "dddd"],
    }));
    const profile = redactProfile(many, 10, "person");
    expect(profile.columns.length).toBe(80);
    expect(profile.columns[0]?.shapes.length).toBeLessThanOrEqual(2);
  });

  it("drops empty shapes rather than sending blanks", () => {
    const profile = redactProfile([{ name: "x", fillRate: 0, samples: ["", "  "] }], 5, "person");
    expect(profile.columns[0]?.shapes).toEqual([]);
  });

  it("carries the entity type, since it decides the attribute vocabulary", () => {
    expect(redactProfile(columns, 400, "account").entity_type).toBe("account");
  });
});

describe("validating the model's proposal", () => {
  const profile: SchemaProfile = {
    entity_type: "person",
    row_count: 100,
    columns: [
      { name: "fnm", fill_rate: 1, distinct_in_sample: 2, shapes: ["a"] },
      { name: "eml", fill_rate: 1, distinct_in_sample: 2, shapes: ["a@a.a"] },
      { name: "city", fill_rate: 1, distinct_in_sample: 2, shapes: ["a"] },
    ],
  };
  const attributes = ["given_name", "email", "addr_city"] as const;

  const reply = (mappings: unknown) => JSON.stringify({ mappings });

  it("accepts a well-formed proposal", () => {
    const out = parseMappings(
      reply([{ attribute: "email", column: "eml", reason: "email-shaped values" }]),
      profile,
      attributes,
      [],
      [],
    );
    expect(out).toEqual([{ attribute: "email", column: "eml", reason: "email-shaped values" }]);
  });

  it("drops a column that is not in the file", () => {
    // A hallucinated name would be written into the source config and fail at
    // INGEST, far from the wizard that caused it.
    const out = parseMappings(
      reply([{ attribute: "email", column: "email_address", reason: "" }]),
      profile,
      attributes,
      [],
      [],
    );
    expect(out).toEqual([]);
  });

  it("drops an attribute outside the entity type's vocabulary", () => {
    // Re-checked here rather than trusted to the schema, the same rule the
    // enrich allowlist follows.
    const out = parseMappings(
      reply([{ attribute: "company_name", column: "fnm", reason: "" }]),
      profile,
      attributes,
      [],
      [],
    );
    expect(out).toEqual([]);
  });

  it("never re-proposes a column the heuristic already claimed", () => {
    // The wizard would otherwise silently overwrite a mapping the person had
    // already confirmed.
    const out = parseMappings(
      reply([{ attribute: "email", column: "eml", reason: "" }]),
      profile,
      attributes,
      ["eml"],
      [],
    );
    expect(out).toEqual([]);
  });

  it("never re-proposes an attribute already assigned", () => {
    const out = parseMappings(
      reply([{ attribute: "email", column: "eml", reason: "" }]),
      profile,
      attributes,
      [],
      ["email"],
    );
    expect(out).toEqual([]);
  });

  it("refuses to map two attributes onto one column", () => {
    const out = parseMappings(
      reply([
        { attribute: "given_name", column: "fnm", reason: "" },
        { attribute: "addr_city", column: "fnm", reason: "" },
      ]),
      profile,
      attributes,
      [],
      [],
    );
    expect(out).toHaveLength(1);
    expect(out[0]?.attribute).toBe("given_name");
  });

  it("refuses two columns for one attribute", () => {
    const out = parseMappings(
      reply([
        { attribute: "given_name", column: "fnm", reason: "" },
        { attribute: "given_name", column: "city", reason: "" },
      ]),
      profile,
      attributes,
      [],
      [],
    );
    expect(out).toHaveLength(1);
    expect(out[0]?.column).toBe("fnm");
  });

  it("returns an empty list rather than throwing on junk", () => {
    // "No suggestions" is a NORMAL outcome — a file whose columns are already
    // mapped, or a model that declined every uncertain one.
    for (const text of ["", "not json", "{}", '{"mappings":"nope"}', "[]"]) {
      expect(parseMappings(text, profile, attributes, [], []), text).toEqual([]);
    }
  });

  it("bounds the reason", () => {
    const out = parseMappings(
      reply([{ attribute: "email", column: "eml", reason: "x".repeat(900) }]),
      profile,
      attributes,
      [],
      [],
    );
    expect(out[0]?.reason.length).toBe(200);
  });
});

describe("the mapping prompt", () => {
  it("explains the shape encoding, or the model reads `aaa@aaa.aaa` as a value", () => {
    const prompt = mappingSystemPrompt(["email"]);
    expect(prompt).toContain("letters shown as `a`");
    expect(prompt).toContain("You never see real values");
  });

  it("frames column names as untrusted data", () => {
    expect(mappingSystemPrompt(["email"])).toContain("untrusted DATA");
  });

  it("asks for omission over a guess, and says why", () => {
    // The reason is load-bearing: a wrong mapping resolves a tenant's records
    // wrongly for as long as the source exists.
    const prompt = mappingSystemPrompt(["email"]);
    expect(prompt).toContain("OMIT anything you are not confident about");
    expect(prompt).toContain("silent and");
  });

  it("names only the attributes still unassigned", () => {
    const prompt = mappingSystemPrompt(["email", "phone"]);
    expect(prompt).toContain("email, phone");
    expect(prompt).not.toContain("given_name");
  });

  it("tells the model which columns are already spoken for", () => {
    const profile: SchemaProfile = { entity_type: "person", row_count: 10, columns: [] };
    expect(mappingUserContent(profile, ["eml", "fnm"])).toContain("must not be reused: eml, fnm");
  });

  it("says nothing about taken columns when there are none", () => {
    const profile: SchemaProfile = { entity_type: "person", row_count: 10, columns: [] };
    expect(mappingUserContent(profile, [])).not.toContain("must not be reused");
  });
});
