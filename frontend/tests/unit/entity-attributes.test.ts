import { describe, expect, it } from "vitest";

import {
  ACCOUNT_GOLDEN_ATTRIBUTES,
  GOLDEN_ATTRIBUTES,
  displayName,
  goldenAttributes,
  recordAttributes,
} from "@/lib/domain/attributes";
import { parseConfig } from "@/lib/domain/config";

describe("entity-type attribute selectors", () => {
  it("defaults to the person golden attributes", () => {
    expect(goldenAttributes()).toBe(GOLDEN_ATTRIBUTES);
    expect(goldenAttributes("person")).toBe(GOLDEN_ATTRIBUTES);
    expect(goldenAttributes("person").map((a) => a.key)).toContain("given_name");
  });

  it("selects the account golden attributes", () => {
    expect(goldenAttributes("account")).toBe(ACCOUNT_GOLDEN_ATTRIBUTES);
    const keys = goldenAttributes("account").map((a) => a.key);
    expect(keys).toContain("company_name");
    expect(keys).toContain("domain");
    expect(keys).not.toContain("given_name");
  });

  it("record attributes carry the entity type's flags", () => {
    expect(recordAttributes("person").map((a) => a.key)).toContain("email_valid");
    expect(recordAttributes("account").map((a) => a.key)).toContain("phone_valid");
    expect(recordAttributes("account").map((a) => a.key)).not.toContain("email_valid");
  });

  it("displayName is given+family for a person, company for an account", () => {
    expect(displayName({ given_name: "Ada", family_name: "Lovelace" })).toBe("Ada Lovelace");
    expect(displayName({ given_name: "Ada", family_name: "Lovelace" }, "person")).toBe(
      "Ada Lovelace",
    );
    expect(displayName({ company_name: "Acme Inc." }, "account")).toBe("Acme Inc.");
    // An account record with no company name, and a person-shaped account lookup,
    // degrade to empty rather than leaking the wrong type's fields.
    expect(displayName({ company_name: null }, "account")).toBe("");
    expect(displayName({ company_name: "Acme" })).toBe("");
  });
});

describe("parseConfig entity_type", () => {
  it("defaults to person when entity_type is absent", () => {
    expect(parseConfig("tenant: acme\n").entityType).toBe("person");
  });

  it("reads an account entity_type", () => {
    expect(parseConfig("tenant: acme\nentity_type: account\n").entityType).toBe("account");
  });

  it("treats an unknown entity_type as person", () => {
    expect(parseConfig("entity_type: widget\n").entityType).toBe("person");
  });
});
