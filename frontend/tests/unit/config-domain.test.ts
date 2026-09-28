import { describe, expect, it } from "vitest";

import {
  addSourceBlock,
  applyEdits,
  diffLines,
  parseConfig,
  tierForEdits,
} from "@/lib/domain/config";

const CONFIG = `tenant: acme
# a load-bearing comment that must survive edits
thresholds:
  auto_merge: 0.95
  review_low: 0.60
blocking:
  - { key_type: email_exact, expr: "email" }
comparisons:
  email: { levels: [exact, username_exact, null], tf: true }
survivorship:
  email: [validated, source_priority, recency]
  given_name: [source_priority, frequency]
sources:
  crm:
    priority_rank: 1
`;

describe("config parsing", () => {
  it("extracts the structured studio view", () => {
    const view = parseConfig(CONFIG);
    expect(view.tenant).toBe("acme");
    expect(view.thresholds).toEqual({ auto_merge: 0.95, review_low: 0.6 });
    expect(view.survivorship.email).toEqual(["validated", "source_priority", "recency"]);
    expect(view.blocking[0]).toEqual({ key_type: "email_exact", expr: "email" });
    expect(view.comparisons.email).toEqual({
      levels: ["exact", "username_exact", "null"],
      tf: true,
    });
    expect(view.sources).toEqual(["crm"]);
  });
});

describe("applying studio edits", () => {
  it("rewrites thresholds while preserving comments and untouched blocks", () => {
    const next = applyEdits(CONFIG, {
      thresholds: { auto_merge: 0.97, review_low: 0.65 },
    });
    expect(next).toContain("auto_merge: 0.97");
    expect(next).toContain("review_low: 0.65");
    expect(next).toContain("# a load-bearing comment that must survive edits");
    expect(next).toContain("key_type: email_exact");
    expect(parseConfig(next).survivorship.email).toEqual([
      "validated",
      "source_priority",
      "recency",
    ]);
  });

  it("reorders a survivorship chain in flow style", () => {
    const next = applyEdits(CONFIG, {
      survivorship: { email: ["recency", "validated"] },
    });
    expect(parseConfig(next).survivorship.email).toEqual(["recency", "validated"]);
    expect(next).toMatch(/email:\s*\[\s*recency,\s*validated\s*\]/);
    // The other chain is untouched.
    expect(parseConfig(next).survivorship.given_name).toEqual(["source_priority", "frequency"]);
  });

  it("adds a wizard source block and refuses duplicates", () => {
    const next = addSourceBlock(CONFIG, "erp", { priority_rank: 2, drop_subdir: "erp" });
    expect(parseConfig(next).sources).toEqual(["crm", "erp"]);
    expect(() => addSourceBlock(next, "erp", {})).toThrow(/already exists/);
  });

  it("labels threshold/survivorship edits tier A", () => {
    expect(tierForEdits({ thresholds: { auto_merge: 0.9, review_low: 0.6 } })).toBe("A");
    expect(tierForEdits({})).toBeNull();
  });
});

describe("line diff", () => {
  it("marks added and removed lines around a stable context", () => {
    const diff = diffLines("a\nb\nc", "a\nB\nc");
    expect(diff).toEqual([
      { kind: "same", text: "a" },
      { kind: "removed", text: "b" },
      { kind: "added", text: "B" },
      { kind: "same", text: "c" },
    ]);
  });
});
