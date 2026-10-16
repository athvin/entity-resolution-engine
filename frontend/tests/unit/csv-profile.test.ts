import { describe, expect, it } from "vitest";

import {
  ACCOUNT_CANONICAL_ATTRIBUTES,
  CANONICAL_ATTRIBUTES,
  canonicalAttributes,
  parseCsv,
  profileCsv,
  suggestIdColumn,
  suggestMapping,
  suggestUpdatedAtColumn,
} from "@/lib/domain/csv-profile";

const CSV = `erp_id,fname,lname,email_address,zip,last_modified
1,Ada,Lovelace,ada@example.com,94103,2026-01-01
2,Grace,,grace@example.com,,2026-01-02
3,"Alan, M",Turing,,94104,2026-01-03
`;

describe("csv parsing", () => {
  it("handles quoted fields with embedded commas", () => {
    const rows = parseCsv(CSV);
    expect(rows[3]?.[1]).toBe("Alan, M");
    expect(rows).toHaveLength(4);
  });

  it("profiles fill rates and samples", () => {
    const { columns, rowCount } = profileCsv(CSV);
    expect(rowCount).toBe(3);
    const lname = columns.find((column) => column.name === "lname");
    expect(lname?.fillRate).toBeCloseTo(2 / 3);
    expect(columns.find((column) => column.name === "email_address")?.samples).toContain(
      "ada@example.com",
    );
  });
});

describe("mapping suggestions", () => {
  const names = CSV.split("\n")[0]?.split(",") ?? [];

  it("maps the obvious aliases onto canonical attributes", () => {
    const mapping = suggestMapping(names);
    expect(mapping.given_name).toBe("fname");
    expect(mapping.family_name).toBe("lname");
    expect(mapping.email).toBe("email_address");
    expect(mapping.addr_postal).toBe("zip");
  });

  it("finds the record id and updated-at columns", () => {
    expect(suggestIdColumn(names)).toBe("erp_id");
    expect(suggestUpdatedAtColumn(names)).toBe("last_modified");
  });
});

describe("account entity-type mapping", () => {
  it("selects the canonical attribute set per entity type", () => {
    expect(canonicalAttributes()).toBe(CANONICAL_ATTRIBUTES);
    expect(canonicalAttributes("person")).toBe(CANONICAL_ATTRIBUTES);
    expect(canonicalAttributes("account")).toBe(ACCOUNT_CANONICAL_ATTRIBUTES);
    expect(canonicalAttributes("account")).toContain("company_name");
    expect(canonicalAttributes("account")).toContain("domain");
    expect(canonicalAttributes("account")).not.toContain("given_name");
  });

  it("maps account aliases onto company/domain/address canonicals", () => {
    const columns = [
      "crm_id",
      "account_name",
      "website",
      "phone_number",
      "street",
      "city",
      "state",
      "zipcode",
    ];
    const mapping = suggestMapping(columns, "account");
    expect(mapping.company_name).toBe("account_name");
    expect(mapping.domain).toBe("website");
    expect(mapping.phone).toBe("phone_number");
    expect(mapping.addr_street).toBe("street");
    expect(mapping.addr_city).toBe("city");
    expect(mapping.addr_region).toBe("state");
    expect(mapping.addr_postal).toBe("zipcode");
    // The account path never invents person fields from an account source.
    expect(mapping.given_name).toBeUndefined();
    expect(mapping.family_name).toBeUndefined();
  });

  it("recognizes 'company'/'domain' spellings and leaves person mapping for the person type", () => {
    const columns = ["company", "domain", "email"];
    expect(suggestMapping(columns, "account").company_name).toBe("company");
    expect(suggestMapping(columns, "account").domain).toBe("domain");
    // The same columns under the person type do not produce a company_name.
    expect(suggestMapping(columns).company_name).toBeUndefined();
    expect(suggestMapping(columns).email).toBe("email");
  });
});
