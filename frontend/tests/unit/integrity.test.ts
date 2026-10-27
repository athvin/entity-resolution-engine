/**
 * Pre-load integrity checks (F2).
 *
 * **Before the upload, not after it.** E9b-1 screens how records landed
 * against the corpus once the dispatcher has them; this answers the earlier
 * question — "will my file work at all" — at the one moment the answer is
 * still cheap, because the file is in the browser and nothing has been
 * written.
 *
 * The severity boundary is what these guard. A `block` refuses the wizard, so
 * a check that blocks a file which would actually have worked teaches people
 * to distrust the gate — and a gate nobody trusts is worse than none.
 */
import { describe, expect, it } from "vitest";

import { checkIntegrity, hasBlocker, type IntegrityInput } from "@/lib/domain/integrity";

const profile = (name: string, fillRate = 1) => ({ name, fillRate, samples: ["x"] });

const input = (over: Partial<IntegrityInput> = {}): IntegrityInput => ({
  columns: [profile("id"), profile("email"), profile("notes")],
  rowCount: 100,
  idColumn: "id",
  mapping: { email: "email" },
  ...over,
});

const of = (findings: ReturnType<typeof checkIntegrity>, severity: string) =>
  findings.filter((finding) => finding.severity === severity);

describe("the file itself", () => {
  it("blocks a file with no header", () => {
    const findings = checkIntegrity(input({ columns: [] }));
    expect(hasBlocker(findings)).toBe(true);
    expect(findings[0]?.message).toContain("no header row");
  });

  it("blocks a header with no rows", () => {
    expect(hasBlocker(checkIntegrity(input({ rowCount: 0 })))).toBe(true);
  });

  it("BLOCKS a duplicated column name", () => {
    // The parser keeps the last, so one column silently shadows another and
    // the mapping points somewhere the reader did not choose.
    const findings = checkIntegrity(
      input({ columns: [profile("id"), profile("email"), profile("email")] }),
    );
    expect(hasBlocker(findings)).toBe(true);
    expect(of(findings, "block")[0]?.message).toContain("more than once");
  });
});

describe("the record id is the column whose problems CORRUPT", () => {
  it("blocks an id column that is not in the file", () => {
    expect(hasBlocker(checkIntegrity(input({ idColumn: "missing" })))).toBe(true);
  });

  it("blocks an id that is blank on some rows", () => {
    // A record with no id cannot be addressed — not by an update, not by a
    // suppression, not by anything.
    const findings = checkIntegrity(
      input({ columns: [profile("id", 0.9), profile("email")], idColumn: "id" }),
    );
    const blocker = of(findings, "block")[0];
    expect(blocker?.message).toContain("cannot be addressed");
    expect(blocker?.rows).toBe(10);
  });

  it("BLOCKS a repeated record id", () => {
    // Two source rows collapse into one and data is lost with no error
    // anywhere — the worst failure this panel exists to prevent.
    const findings = checkIntegrity(
      input({
        header: ["id", "email"],
        rows: [
          ["A", "a@x.com"],
          ["B", "b@x.com"],
          ["A", "c@x.com"],
        ],
      }),
    );
    const blocker = of(findings, "block").find((f) => f.message.includes("repeats"));
    expect(blocker).toBeDefined();
    expect(blocker?.rows).toBe(1);
  });

  it("does not count blank ids as duplicates of each other", () => {
    // They are already blocked for being blank; counting them twice would
    // report two problems where there is one.
    const findings = checkIntegrity(
      input({
        header: ["id", "email"],
        rows: [
          ["", "a@x.com"],
          ["", "b@x.com"],
        ],
      }),
    );
    expect(of(findings, "block").some((f) => f.message.includes("repeats"))).toBe(false);
  });

  it("says nothing about ids when no id column is chosen yet", () => {
    const findings = checkIntegrity(input({ idColumn: "" }));
    expect(of(findings, "block")).toEqual([]);
  });
});

describe("a mapped column that is empty is a WARN, not a block", () => {
  it("warns when a mapped column is empty on every row", () => {
    // It will load; it just will not do what the person expects. Blocking a
    // file that would have worked is how a gate loses trust.
    const findings = checkIntegrity(
      input({ columns: [profile("id"), profile("email", 0)], mapping: { email: "email" } }),
    );
    expect(hasBlocker(findings)).toBe(false);
    expect(of(findings, "warn")[0]?.message).toContain("empty on every row");
  });

  it("warns on a barely-populated column", () => {
    const findings = checkIntegrity(
      input({ columns: [profile("id"), profile("email", 0.05)], mapping: { email: "email" } }),
    );
    expect(of(findings, "warn")[0]?.message).toContain("under 10%");
  });

  it("stays quiet on a well-populated one", () => {
    const findings = checkIntegrity(
      input({ columns: [profile("id"), profile("email", 0.8)], mapping: { email: "email" } }),
    );
    expect(of(findings, "warn")).toEqual([]);
  });

  it("BLOCKS a mapping pointing at a column that is not there", () => {
    // Not a warn: the ingest would fail outright.
    const findings = checkIntegrity(input({ mapping: { email: "nope" } }));
    expect(hasBlocker(findings)).toBe(true);
  });
});

describe("unmapped columns are information, not a problem", () => {
  it("says they are kept as metadata rather than dropped", () => {
    const findings = checkIntegrity(input());
    const note = of(findings, "info")[0];
    expect(note?.message).toContain("kept as metadata");
    expect(note?.rows).toBe(1); // `notes`
  });

  it("does not count the id column as unmapped", () => {
    const findings = checkIntegrity(
      input({ columns: [profile("id"), profile("email")], mapping: { email: "email" } }),
    );
    expect(of(findings, "info")).toEqual([]);
  });
});

describe("ordering", () => {
  it("puts blockers first, so the thing stopping you is at the top", () => {
    const findings = checkIntegrity(
      input({
        columns: [profile("id", 0.5), profile("email", 0), profile("notes")],
        mapping: { email: "email" },
      }),
    );
    expect(findings[0]?.severity).toBe("block");
    expect(findings[findings.length - 1]?.severity).toBe("info");
  });
});

describe("the row-level checks say they are a SAMPLE", () => {
  it("never claims a clean file, only clean sampled rows", () => {
    // `profileCsv` parses the first 200 rows. Reporting "no duplicate ids"
    // from that as a guarantee over a million-row file would be exactly the
    // false assurance this panel exists to avoid.
    const findings = checkIntegrity(
      input({
        header: ["id", "email"],
        rows: [
          ["A", "a@x.com"],
          ["A", "b@x.com"],
        ],
      }),
    );
    const blocker = findings.find((f) => f.message.includes("repeats"));
    expect(blocker?.message).toContain("in the sampled rows");
  });
});
