/**
 * Config promotion between orgs (O9).
 *
 * **The authorization rule is the whole feature**, and it is the one thing a
 * unit test here cannot prove — `requireOrgAccess` is called twice in the
 * route, and that it is called twice is asserted by the BFF authz gate plus the
 * functional spec. What this file guards is everything around it: the request
 * shapes, and the self-promotion refusal that would otherwise leave a draft
 * nobody meant to make.
 */
import { describe, expect, it } from "vitest";

import { isSameOrg, parseCompare, parseImport } from "@/lib/bff/promote";

describe("comparing against another org", () => {
  it("accepts an org name, with or without a version", () => {
    expect(parseCompare({ against: "staging-dev" }).ok).toBe(true);
    const pinned = parseCompare({ against: "staging-dev", version: 3 });
    expect(pinned.ok).toBe(true);
    if (pinned.ok) expect(pinned.value.version).toBe(3);
  });

  it("requires the other org", () => {
    const parsed = parseCompare({});
    expect(parsed.ok).toBe(false);
    if (!parsed.ok) expect(parsed.reason).toContain("against");
  });

  it("refuses a blank or oversized org name", () => {
    expect(parseCompare({ against: "" }).ok).toBe(false);
    expect(parseCompare({ against: "   " }).ok).toBe(false);
    expect(parseCompare({ against: "x".repeat(65) }).ok).toBe(false);
  });

  it("refuses a version that is not a positive integer", () => {
    for (const version of [0, -1, 1.5, "2"]) {
      expect(parseCompare({ against: "o", version }).ok, String(version)).toBe(false);
    }
  });
});

describe("importing another org's config", () => {
  it("accepts a source org", () => {
    const parsed = parseImport({ from: "staging-dev", version: 2 });
    expect(parsed.ok).toBe(true);
    if (parsed.ok) expect(parsed.value.from).toBe("staging-dev");
  });

  it("requires it", () => {
    const parsed = parseImport({ version: 2 });
    expect(parsed.ok).toBe(false);
    if (!parsed.ok) expect(parsed.reason).toContain("from");
  });

  it("trims, so a pasted org name with whitespace still resolves", () => {
    const parsed = parseImport({ from: "  staging-dev  " });
    expect(parsed.ok && parsed.value.from).toBe("staging-dev");
  });
});

describe("promotion to self is refused", () => {
  it("recognises the same org", () => {
    // Harmless in effect — the draft would equal the published version — but
    // always a mistake, and it leaves behind something that reads as a pending
    // change nobody made.
    expect(isSameOrg("acme-dev", "acme-dev")).toBe(true);
  });

  it("ignores case and surrounding whitespace", () => {
    // Org names are slugs; a caller pasting one with different case is making
    // the same mistake, and the refusal has to recognise it as such.
    expect(isSameOrg("acme-dev", "ACME-DEV")).toBe(true);
    expect(isSameOrg("  acme-dev", "acme-dev  ")).toBe(true);
  });

  it("does not confuse two different orgs", () => {
    expect(isSameOrg("acme-dev", "acme-prod")).toBe(false);
    expect(isSameOrg("acme", "acme-dev")).toBe(false);
  });
});
