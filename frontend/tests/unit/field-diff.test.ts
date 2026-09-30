import { describe, expect, it } from "vitest";

import { GOLDEN_ATTRIBUTES } from "@/lib/domain/attributes";
import { differingFields, diffFields } from "@/lib/domain/field-diff";

const NAME_ATTRS = GOLDEN_ATTRIBUTES.filter((a) =>
  ["given_name", "family_name", "email"].includes(a.key),
);

describe("diffFields", () => {
  it("compares descriptor by descriptor in order", () => {
    const rows = diffFields(
      NAME_ATTRS,
      { given_name: "Bob", family_name: "Chen", email: "b@x.co" },
      { given_name: "Robert", family_name: "Chen", email: "b@x.co" },
    );
    expect(rows.map((row) => [row.key, row.same])).toEqual([
      ["given_name", false],
      ["family_name", true],
      ["email", true],
    ]);
    expect(rows[0]).toMatchObject({ a: "Bob", b: "Robert", label: "Given name" });
  });

  it("treats every spelling of absence as the same non-value", () => {
    const rows = diffFields(
      NAME_ATTRS,
      { given_name: null, family_name: "", email: undefined },
      { given_name: "", family_name: null, email: "   " },
    );
    expect(rows.every((row) => row.same)).toBe(true);
    expect(rows.map((row) => row.a)).toEqual([null, null, null]);
  });

  it("renders booleans and trims text before comparing", () => {
    const flag = [{ key: "email_valid", label: "Email valid", kind: "flag" as const }];
    const rows = diffFields(flag, { email_valid: true }, { email_valid: false });
    expect(rows[0]).toMatchObject({ a: "true", b: "false", same: false });

    const padded = diffFields(NAME_ATTRS, { given_name: " Bob " }, { given_name: "Bob" });
    expect(padded[0]?.same).toBe(true);
  });

  it("differingFields drops agreements including both-empty rows", () => {
    const rows = diffFields(
      NAME_ATTRS,
      { given_name: "Bob", family_name: null, email: "a@b.c" },
      { given_name: "Bob", family_name: null, email: "z@b.c" },
    );
    expect(differingFields(rows).map((row) => row.key)).toEqual(["email"]);
  });
});
