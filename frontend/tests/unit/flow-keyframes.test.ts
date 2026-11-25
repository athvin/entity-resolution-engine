import { describe, expect, it } from "vitest";

import { simplifySamples, type FlowSample } from "@/components/marketing/demo/flow-keyframes";

/** A sprite crossing an L-shaped trail with a rounded corner, sampled every unit. */
function trail(): FlowSample[] {
  const out: FlowSample[] = [];
  const n = 600;
  for (let i = 0; i <= n; i++) {
    const p = i / n;
    const d = p * 400; // 200 right, then 200 down; quarter-circle corner of radius 12
    let x: number;
    let y: number;
    if (d <= 188) [x, y] = [d, 0];
    else if (d >= 188 + (Math.PI * 12) / 2) [x, y] = [200, 12 + (d - 188 - (Math.PI * 12) / 2)];
    else {
      const a = (d - 188) / 12;
      [x, y] = [188 + 12 * Math.sin(a), 12 - 12 * Math.cos(a)];
    }
    out.push({ offset: 0.1 + 0.8 * p, x, y, opacity: Math.min(1, p / 0.025, (1 - p) / 0.025) });
  }
  return out;
}

/** Position of the simplified motion at `offset`, as the browser interpolates it. */
function at(frames: FlowSample[], offset: number) {
  const j = frames.findIndex((f) => f.offset >= offset);
  const b = frames[j];
  const a = frames[Math.max(0, j - 1)];
  if (!a || !b) throw new Error("offset out of range");
  const u = b.offset === a.offset ? 0 : (offset - a.offset) / (b.offset - a.offset);
  return {
    x: a.x + (b.x - a.x) * u,
    y: a.y + (b.y - a.y) * u,
    opacity: a.opacity + (b.opacity - a.opacity) * u,
  };
}

describe("simplifySamples", () => {
  it("keeps a straight constant-speed run as its two ends", () => {
    const line = Array.from({ length: 300 }, (_, i) => ({
      offset: i / 299,
      x: i * 2,
      y: i,
      opacity: 1,
    }));
    expect(simplifySamples(line)).toEqual([line[0], line[299]]);
  });

  it("cuts a sampled trail by an order of magnitude without moving the sprite", () => {
    const samples = trail();
    const kept = simplifySamples(samples);
    expect(kept.length).toBeLessThan(samples.length / 10);
    expect(kept[0]).toBe(samples[0]);
    expect(kept.at(-1)).toBe(samples.at(-1));
    for (const s of samples) {
      const p = at(kept, s.offset);
      expect(Math.hypot(p.x - s.x, p.y - s.y)).toBeLessThanOrEqual(0.5 + 1e-9);
      expect(Math.abs(p.opacity - s.opacity)).toBeLessThanOrEqual(0.01 + 1e-9);
    }
  });

  it("passes through inputs too short to simplify", () => {
    const one = [{ offset: 0, x: 0, y: 0, opacity: 0 }];
    expect(simplifySamples(one)).toEqual(one);
    expect(simplifySamples([])).toEqual([]);
  });
});
