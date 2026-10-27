/**
 * Field-value history (O12c).
 *
 * **No new table.** DuckLake keeps every version of `golden_records`, and the
 * run ledger records the snapshot range each run produced — so "what did this
 * field used to be" is a read at an older lake version. The history already
 * existed; it was simply never reachable.
 *
 * What these guard is which snapshots may be OFFERED. A reader restoring a
 * value trusts that the version they picked is a state the product actually
 * published, and the two filters below are what make that true.
 */
import { describe, expect, it } from "vitest";

import { historyPoints, valueChanged, type RunLike } from "@/lib/history/snapshots";

const run = (over: Partial<RunLike> = {}): RunLike => ({
  id: "01RUN1",
  status: "completed",
  ended_at: "2026-10-01T10:00:00Z",
  snapshot_end: 7,
  ...over,
});

describe("which snapshots may be offered", () => {
  it("offers a completed run's end snapshot", () => {
    expect(historyPoints([run()])).toEqual([
      { snapshot: 7, runId: "01RUN1", endedAt: "2026-10-01T10:00:00Z" },
    ]);
  });

  it("NEVER offers a failed or running run", () => {
    // A partial snapshot range is a state the product never published, and
    // restoring "as of" a half-finished run would resurrect it.
    for (const status of ["failed", "running", "queued"]) {
      expect(historyPoints([run({ status })]), status).toEqual([]);
    }
    // A run with no status at all is the same refusal, spelled separately
    // because `exactOptionalPropertyTypes` will not take an explicit undefined.
    const noStatus: RunLike = { id: "01RUN1", ended_at: "2026-10-01T10:00:00Z", snapshot_end: 7 };
    expect(historyPoints([noStatus])).toEqual([]);
  });

  it("skips a run that produced no snapshot", () => {
    expect(historyPoints([run({ snapshot_end: null })])).toEqual([]);
    expect(historyPoints([run({ snapshot_end: undefined })])).toEqual([]);
  });

  it("reads a snapshot that arrives as a string", () => {
    // The lake's bigints reach the wire as strings through some drivers; the
    // same lesson O12b's `sum` taught.
    expect(historyPoints([run({ snapshot_end: "12" })])[0]?.snapshot).toBe(12);
  });

  it("refuses a non-numeric snapshot rather than coercing it", () => {
    for (const bad of ["latest", "", "7.5", {}, true]) {
      expect(historyPoints([run({ snapshot_end: bad })]), JSON.stringify(bad)).toEqual([]);
    }
  });

  it("de-duplicates, since several runs can end on the same version", () => {
    // A run that changed nothing ends where the last one did. Two identical
    // entries is a choice with no difference.
    const points = historyPoints([
      run({ id: "01RUN2", snapshot_end: 7 }),
      run({ id: "01RUN1", snapshot_end: 7 }),
    ]);
    expect(points).toHaveLength(1);
  });

  it("orders newest first", () => {
    const points = historyPoints([
      run({ id: "a", snapshot_end: 3 }),
      run({ id: "b", snapshot_end: 9 }),
      run({ id: "c", snapshot_end: 6 }),
    ]);
    expect(points.map((point) => point.snapshot)).toEqual([9, 6, 3]);
  });

  it("bounds how many it offers", () => {
    const many = Array.from({ length: 50 }, (_, i) =>
      run({ id: `r${String(i)}`, snapshot_end: i + 1 }),
    );
    expect(historyPoints(many)).toHaveLength(10);
    expect(historyPoints(many, 3)).toHaveLength(3);
  });
});

describe("whether a value actually changed", () => {
  it("sees a real change", () => {
    expect(valueChanged("Bob", "Robert")).toBe(true);
  });

  it("treats absence and empty as the same", () => {
    // A field never set and one set to "" are indistinguishable to a reader;
    // counting that as a change fills the history with differences nobody made.
    expect(valueChanged(null, "")).toBe(false);
    expect(valueChanged(undefined, "   ")).toBe(false);
    expect(valueChanged("", null)).toBe(false);
  });

  it("ignores surrounding whitespace", () => {
    expect(valueChanged("Bob", "  Bob  ")).toBe(false);
  });

  it("sees a value appearing or disappearing", () => {
    expect(valueChanged(null, "Bob")).toBe(true);
    expect(valueChanged("Bob", null)).toBe(true);
  });
});
