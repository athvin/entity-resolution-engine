/**
 * Verify-before-export (F4).
 *
 * The need behind #86: do not send a campaign to records you cannot reach. An
 * export of 10,000 rows where 3,000 have no email is not a segment, it is a
 * 30% bounce rate someone discovers later.
 *
 * **This is a completeness check on the exported columns, not a read of V3's
 * `record_quality` scores.** Those are real and built, but scored per SOURCE
 * RECORD while an export is per ENTITY — using them wants a declared read
 * joining golden → members → quality, which is a follow-up rather than
 * something to smuggle in here.
 */
import { describe, expect, it } from "vitest";

import {
  EXPORT_REQUIREMENTS,
  exportSummary,
  isExportRequirement,
  isExportable,
} from "@/lib/bff/exportable";

const row = (over: Record<string, unknown> = {}) => ({
  id: "E1",
  email: "ada@example.com",
  phone_e164: "+14155550001",
  addr_postal: "94103",
  ...over,
});

describe("the default must not change existing exports", () => {
  it("`any` exports everything", () => {
    // Adding an option that silently filtered would change what every saved
    // export produces, with no diff anyone would see.
    expect(isExportable(row({ email: null, phone_e164: null, addr_postal: null }), "any")).toBe(
      true,
    );
  });

  it("is the first requirement, so it is the schema default", () => {
    expect(EXPORT_REQUIREMENTS[0]).toBe("any");
  });
});

describe("each requirement reads its own column", () => {
  it("keeps a row that has the required value", () => {
    expect(isExportable(row(), "email")).toBe(true);
    expect(isExportable(row(), "phone")).toBe(true);
    expect(isExportable(row(), "postal")).toBe(true);
  });

  it("drops a row missing it", () => {
    expect(isExportable(row({ email: null }), "email")).toBe(false);
    expect(isExportable(row({ phone_e164: undefined }), "phone")).toBe(false);
    expect(isExportable(row({ addr_postal: "" }), "postal")).toBe(false);
  });

  it("does not confuse one column for another", () => {
    // A row with a phone but no email must fail the email requirement — the
    // bug that would make a "verified" export bounce.
    expect(isExportable(row({ email: null }), "phone")).toBe(true);
    expect(isExportable(row({ email: null }), "email")).toBe(false);
  });

  it("treats whitespace as absence", () => {
    // `" "` is indistinguishable from empty to whoever receives the file.
    expect(isExportable(row({ email: "   " }), "email")).toBe(false);
    expect(isExportable(row({ email: "\t\n" }), "email")).toBe(false);
  });

  it("accepts a numeric postal code", () => {
    // Postal codes arrive as numbers from some sources; dropping them would
    // silently exclude whole countries.
    expect(isExportable(row({ addr_postal: 94103 }), "postal")).toBe(true);
  });
});

describe("the requirement vocabulary is closed", () => {
  it("accepts exactly the four", () => {
    for (const value of EXPORT_REQUIREMENTS) expect(isExportRequirement(value)).toBe(true);
  });

  it("refuses anything else", () => {
    for (const value of ["address", "Email", "", "valid"]) {
      expect(isExportRequirement(value), value).toBe(false);
    }
  });
});

describe("the summary reports what was LEFT OUT", () => {
  it("says so when rows were skipped", () => {
    // An export that silently returns 7,000 of 10,000 looks like a broken
    // query, and the reader's next move is to re-run rather than trust it.
    expect(exportSummary(7_000, 3_000, "email")).toContain("3000 skipped");
  });

  it("says plainly when nothing was skipped", () => {
    expect(exportSummary(10, 0, "email")).toContain("all with an email");
  });

  it("mentions no requirement for an unfiltered export", () => {
    const summary = exportSummary(10, 0, "any");
    expect(summary).toBe("10 rows");
    expect(summary).not.toContain("skipped");
  });

  it("reads naturally for a postal code", () => {
    expect(exportSummary(5, 1, "postal")).toContain("a postal code");
  });
});
