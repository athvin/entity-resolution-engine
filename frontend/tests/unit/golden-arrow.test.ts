import { tableFromIPC } from "apache-arrow";
import { describe, expect, it } from "vitest";

import { readAcctGoldenRecord } from "@/lib/entities/readers/AcctGoldenRecord";
import { readGoldenRecord } from "@/lib/entities/readers/GoldenRecord";

import { encodeAccountGolden, encodePersonGolden } from "../mock-erserver/arrow.mjs";

// Proves the mock's Arrow encoder and the generated readers agree on the wire:
// the type-sensitive columns (Date32 birth_date, Timestamp(µs) assembled_at)
// decode correctly after an IPC round-trip. This is what the backend streams and
// the records browser consumes, validated without Playwright.
describe("golden-records Arrow round-trip (mock encoder ↔ generated reader)", () => {
  const person = [
    {
      entity_id: "01jm0e000000",
      given_name: "Ada",
      family_name: "Lovelace",
      email: "ada@example.test",
      phone_e164: "+14155551000",
      addr_number: "100",
      addr_street: "Market St",
      addr_unit: null,
      addr_city: "San Francisco",
      addr_region: "CA",
      addr_postal: "94103",
      birth_date: "1970-01-01",
      survivorship_version: 1,
      assembled_at: "2026-09-24T06:01:00Z",
    },
    {
      entity_id: "01jm0e000001",
      given_name: "Grace",
      family_name: "Hopper",
      email: "grace@example.test",
      phone_e164: "+14155551001",
      addr_number: "101",
      addr_street: "Market St",
      addr_unit: null,
      addr_city: "San Francisco",
      addr_region: "CA",
      addr_postal: "94103",
      birth_date: "1906-12-09",
      survivorship_version: 1,
      assembled_at: "2026-09-24T06:01:00Z",
    },
  ];

  it("decodes a person page through readGoldenRecord", () => {
    const table = tableFromIPC(encodePersonGolden(person));
    expect(table.schema.metadata.get("er.relation")).toBe("golden_records");
    const rows = readGoldenRecord(table);
    expect(rows).toHaveLength(2);
    expect(rows[0]?.entity_id).toBe("01jm0e000000");
    expect(rows[0]?.given_name).toBe("Ada");
    expect(rows[0]?.email).toBe("ada@example.test");
    expect(rows[0]?.addr_unit).toBeNull();
    // The fragile columns: Date32 → ISO date, Timestamp(µs) → ISO instant.
    expect(rows[0]?.birth_date).toBe("1970-01-01");
    expect(rows[1]?.birth_date).toBe("1906-12-09");
    expect(rows[0]?.assembled_at).toBe("2026-09-24T06:01:00.000Z");
    // No metadata in the fixtures → a tolerated null.
    expect(rows[0]?.metadata).toBeNull();
    expect(rows[0]?.survivorship_version).toBe("1");
  });

  it("decodes an account page through readAcctGoldenRecord", () => {
    const account = [
      {
        entity_id: "01jm0a000000",
        company_name: "Acme Inc.",
        domain: "acme.example",
        phone_e164: "+14155552000",
        addr_street: "100 Market St",
        addr_city: "San Francisco",
        addr_region: "CA",
        addr_postal: "94103",
        survivorship_version: 1,
        assembled_at: "2026-09-24T06:01:00Z",
      },
    ];
    const table = tableFromIPC(encodeAccountGolden(account));
    expect(table.schema.metadata.get("er.relation")).toBe("acct_golden_records");
    const rows = readAcctGoldenRecord(table);
    expect(rows).toHaveLength(1);
    expect(rows[0]?.company_name).toBe("Acme Inc.");
    expect(rows[0]?.domain).toBe("acme.example");
    expect(rows[0]?.assembled_at).toBe("2026-09-24T06:01:00.000Z");
  });

  it("encodes an empty page as a schema-only stream (zero rows, no throw)", () => {
    const table = tableFromIPC(encodePersonGolden([]));
    expect(table.schema.metadata.get("er.relation")).toBe("golden_records");
    expect(readGoldenRecord(table)).toHaveLength(0);
  });
});
