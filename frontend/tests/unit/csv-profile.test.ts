import { describe, expect, it } from "vitest";

import {
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
