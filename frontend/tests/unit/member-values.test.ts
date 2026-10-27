/**
 * The group merge editor's comparison (F1).
 *
 * The review inbox answers "are these the same"; this answers what comes
 * after — the group is right, but is the golden record holding the best value
 * from it?
 *
 * **The rule that keeps it safe is `hasAlternative`.** A member whose field is
 * EMPTY differs from a populated master, but offering that as an adoptable
 * value would turn a one-click control into a silent delete. That is the
 * destructive mistake this surface could most easily make, and the only thing
 * preventing it is that one predicate.
 */
import { describe, expect, it } from "vitest";

import { compareAcrossMembers } from "@/lib/query/member-values";
import type { SourceRecord } from "@/lib/query/reviews";

const member = (key: string, attributes: Record<string, unknown>): SourceRecord => ({
  record_key: key,
  source_system: key.split(":")[0] ?? "crm",
  source_record_id: key.split(":")[1] ?? "1",
  attributes,
});

const attributes = ["given_name", "email", "phone_e164"];

describe("what differs from the master", () => {
  it("marks a member value that differs", () => {
    const fields = compareAcrossMembers(
      attributes,
      { given_name: "Robert", email: "r@x.com", phone_e164: null },
      [member("crm:1", { given_name: "Bob", email: "r@x.com" })],
    );
    const name = fields.find((field) => field.attribute === "given_name");
    expect(name?.values[0]?.differs).toBe(true);
    expect(name?.hasAlternative).toBe(true);
  });

  it("does not mark one that agrees", () => {
    const fields = compareAcrossMembers(attributes, { email: "r@x.com" }, [
      member("crm:1", { email: "r@x.com" }),
    ]);
    const email = fields.find((field) => field.attribute === "email");
    expect(email?.values[0]?.differs).toBe(false);
    expect(email?.hasAlternative).toBe(false);
  });

  it("compares against the MASTER, not between members", () => {
    // Two members disagreeing with each other while both matching the master
    // is not something a reader needs flagged.
    const fields = compareAcrossMembers(["email"], { email: "r@x.com" }, [
      member("crm:1", { email: "r@x.com" }),
      member("crm:2", { email: "r@x.com" }),
    ]);
    expect(fields[0]?.hasAlternative).toBe(false);
  });
});

describe("an EMPTY member value is never an alternative", () => {
  it("does not offer a missing value as adoptable", () => {
    // THE guard. Adopting it would clear the golden field — a one-click
    // silent delete.
    const fields = compareAcrossMembers(["email"], { email: "r@x.com" }, [member("crm:1", {})]);
    expect(fields[0]?.values[0]?.differs).toBe(true);
    expect(fields[0]?.hasAlternative).toBe(false);
  });

  it("treats whitespace as empty", () => {
    const fields = compareAcrossMembers(["email"], { email: "r@x.com" }, [
      member("crm:1", { email: "   " }),
    ]);
    expect(fields[0]?.hasAlternative).toBe(false);
  });

  it("DOES offer a value where the master is the empty one", () => {
    // The useful direction: the golden record is missing something a member
    // has, and adopting it fills a gap rather than creating one.
    const fields = compareAcrossMembers(["email"], { email: null }, [
      member("crm:1", { email: "r@x.com" }),
    ]);
    expect(fields[0]?.hasAlternative).toBe(true);
  });
});

describe("normalisation", () => {
  it("ignores surrounding whitespace when comparing", () => {
    const fields = compareAcrossMembers(["email"], { email: "r@x.com" }, [
      member("crm:1", { email: "  r@x.com  " }),
    ]);
    expect(fields[0]?.values[0]?.differs).toBe(false);
  });

  it("renders a numeric value as text rather than dropping it", () => {
    // Postal codes arrive as numbers from some sources; dropping them would
    // make a populated field look empty and offer a delete.
    const fields = compareAcrossMembers(["addr_postal"], { addr_postal: "94103" }, [
      member("crm:1", { addr_postal: 94103 }),
    ]);
    expect(fields[0]?.values[0]?.value).toBe("94103");
    expect(fields[0]?.values[0]?.differs).toBe(false);
  });

  it("returns a row per attribute even when no member has it", () => {
    const fields = compareAcrossMembers(attributes, {}, [member("crm:1", {})]);
    expect(fields).toHaveLength(3);
    expect(fields.every((field) => !field.hasAlternative)).toBe(true);
  });
});
