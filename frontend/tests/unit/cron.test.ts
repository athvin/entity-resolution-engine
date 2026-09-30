import { describe, expect, it } from "vitest";

import { cronPresets, formatFire, nextFires, parseCron } from "@/lib/domain/cron";

// A fixed Tuesday: 2026-09-29 10:30:00 UTC. Every expectation is deterministic.
const NOW = new Date(Date.UTC(2026, 8, 29, 10, 30, 0));

function fires(expr: string, count = 3): string[] {
  return nextFires(expr, NOW, count).map((at) => at.toISOString());
}

describe("parseCron", () => {
  it("accepts the portable grammar and rejects strangers", () => {
    for (const good of ["0 6 * * *", "*/15 * * * *", "0 4 * * 0", "30 2 1,15 * *", "0 0 1-7 * 1"]) {
      expect(parseCron(good), good).not.toBeNull();
    }
    for (const bad of ["0 6 * *", "60 * * * *", "* * 0 * *", "a b c d e", "0 6 * * 8/"]) {
      expect(parseCron(bad), bad).toBeNull();
    }
  });

  it("treats 7 as Sunday's second spelling", () => {
    expect(fires("0 4 * * 7", 1)).toEqual(fires("0 4 * * 0", 1));
  });
});

describe("nextFires", () => {
  it("finds the next daily fires strictly after now", () => {
    expect(fires("0 6 * * *")).toEqual([
      "2026-09-30T06:00:00.000Z",
      "2026-10-01T06:00:00.000Z",
      "2026-10-02T06:00:00.000Z",
    ]);
  });

  it("fires later the same day when the slot is still ahead", () => {
    expect(fires("45 10 * * *", 1)).toEqual(["2026-09-29T10:45:00.000Z"]);
  });

  it("handles steps and weekly slots", () => {
    expect(fires("*/15 * * * *", 2)).toEqual([
      "2026-09-29T10:45:00.000Z",
      "2026-09-29T11:00:00.000Z",
    ]);
    // Next Sunday after Tue 2026-09-29 is 2026-10-04.
    expect(fires("0 4 * * 0", 1)).toEqual(["2026-10-04T04:00:00.000Z"]);
  });

  it("applies the OR rule when both day fields are restricted", () => {
    // dom=1 OR dow=Wednesday(3): Wed 2026-09-30 beats Oct 1.
    expect(fires("0 0 1 * 3", 2)).toEqual(["2026-09-30T00:00:00.000Z", "2026-10-01T00:00:00.000Z"]);
  });

  it("returns [] for an invalid expression", () => {
    expect(fires("not cron")).toEqual([]);
  });
});

describe("presets and rendering", () => {
  it("composes the four preset shapes", () => {
    expect(cronPresets.hourly(15)).toBe("15 * * * *");
    expect(cronPresets.daily(6, 0)).toBe("0 6 * * *");
    expect(cronPresets.weekly(0, 4, 0)).toBe("0 4 * * 0");
    expect(cronPresets.monthly(1, 2, 30)).toBe("30 2 1 * *");
  });

  it("renders a fire time in UTC", () => {
    expect(formatFire(new Date(Date.UTC(2026, 8, 30, 6, 0)))).toBe("Wed 2026-09-30 06:00 UTC");
  });
});
