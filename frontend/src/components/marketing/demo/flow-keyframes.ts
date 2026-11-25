/** One sampled position of a sprite along its path. */
export type FlowSample = { offset: number; x: number; y: number; opacity: number };

/** Largest drift a dropped sample may introduce: half a CSS pixel, 1% opacity. */
const MAX_PX = 0.5;
const MAX_OPACITY = 0.01;

/**
 * Drop every sample that linear interpolation between its neighbours already
 * reproduces, so a sprite's keyframes describe the path's corners rather than
 * every 4px of it.
 *
 * The motion is `easing: "linear"` and offsets are proportional to distance, so
 * on a straight run the browser interpolates the dropped positions exactly; only
 * bends and the opacity ramps need intermediate keyframes. This is
 * Ramer–Douglas–Peucker over (time, x, y, opacity), with interpolation by
 * offset rather than by nearest point, because the sprite must still be in the
 * same place at the same time.
 *
 * Why it matters: the trails sampled up to 600 keyframes per sprite and the
 * home page runs dozens of sprites. WebKit re-resolves those effects on every
 * scroll frame, paused or not, and a 700ms scroll there rendered in 3–5 frames
 * of ~300ms each. With the corners alone it holds 60fps.
 */
export function simplifySamples(samples: readonly FlowSample[]): FlowSample[] {
  if (samples.length <= 2) return [...samples];
  const keep = new Uint8Array(samples.length);
  keep[0] = 1;
  keep[samples.length - 1] = 1;
  const stack: [number, number][] = [[0, samples.length - 1]];
  for (let range = stack.pop(); range; range = stack.pop()) {
    const [from, to] = range;
    const a = samples[from];
    const b = samples[to];
    if (!a || !b || to - from < 2) continue;
    const span = b.offset - a.offset;
    let worst = 0;
    let split = -1;
    for (let index = from + 1; index < to; index++) {
      const s = samples[index];
      if (!s) continue;
      const u = span > 0 ? (s.offset - a.offset) / span : 0;
      const dx = a.x + (b.x - a.x) * u - s.x;
      const dy = a.y + (b.y - a.y) * u - s.y;
      const dOpacity = Math.abs(a.opacity + (b.opacity - a.opacity) * u - s.opacity);
      const error = Math.max(Math.hypot(dx, dy) / MAX_PX, dOpacity / MAX_OPACITY);
      if (error > worst) {
        worst = error;
        split = index;
      }
    }
    if (worst > 1 && split > 0) {
      keep[split] = 1;
      stack.push([from, split], [split, to]);
    }
  }
  return samples.filter((_, index) => keep[index] === 1);
}
