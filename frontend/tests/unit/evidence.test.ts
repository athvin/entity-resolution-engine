import { describe, expect, it } from "vitest";

import { describeRow, parseEvidence } from "@/lib/domain/evidence";

const BLOB = {
  gamma_email: 2,
  mw_email: 4.1,
  gamma_birth_date: 0,
  mw_birth_date: -2.3,
  gamma_family_name: 2,
  mw_family_name: 1.8,
  mw_tf_adj_family_name: 0.4,
  match_weight: 4.0,
};

describe("evidence parsing", () => {
  it("folds the flat blob into per-comparison rows, largest magnitude first", () => {
    const evidence = parseEvidence(BLOB);
    expect(evidence.totalWeight).toBe(4.0);
    expect(evidence.rows.map((row) => row.column)).toEqual(["email", "birth_date", "family_name"]);
  });

  it("folds tf adjustments into the column weight", () => {
    const family = parseEvidence(BLOB).rows.find((row) => row.column === "family_name");
    expect(family?.weight).toBeCloseTo(2.2);
    expect(family?.supports).toBe(true);
  });

  it("marks negative weights as arguing against", () => {
    const birth = parseEvidence(BLOB).rows.find((row) => row.column === "birth_date");
    if (!birth) throw new Error("birth_date row missing");
    expect(birth.supports).toBe(false);
    expect(describeRow(birth)).toBe("birth date: disagree — argues against (-2.3)");
  });

  it("tolerates an empty or missing blob", () => {
    expect(parseEvidence(null).rows).toEqual([]);
    expect(parseEvidence({}).totalWeight).toBeNull();
  });

  it("ignores junk values without crashing", () => {
    const evidence = parseEvidence({ mw_email: "not-a-number", gamma_email: 2 });
    expect(evidence.rows).toHaveLength(1);
    expect(evidence.rows[0]?.weight).toBe(0);
  });
});
